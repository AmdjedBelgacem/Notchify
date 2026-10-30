//! Accessibility fallback: observe Notification Center banners.
//!
//! **Fragile — isolated here.** When Full Disk Access is not granted (or the
//! usernoted DB schema changes), NotchForge can still see notifications by
//! watching the Notification Center UI process (`com.apple.notificationcenterui`)
//! for new windows and reading the static texts inside them. The AX tree of
//! banners is undocumented and changes between macOS releases, so:
//!
//! * text extraction is heuristic (first text = title, the rest = body),
//! * app attribution uses the banner's accessibility description and falls
//!   back to text hints,
//! * the optional "dismiss native banner" feature performs the banner's own
//!   *Close* action and is off by default.
//!
//! Requires the Accessibility permission.

use crate::ffi::ax::*;
use crate::ffi::cf::{self, CFArrayGetCount, CFArrayGetValueAtIndex, CFTypeRef, Owned};
use notchforge_core::apps::{AppRef, KnownApp};
use notchforge_core::model::{NewNotification, Notification, NotificationKind, Origin};
use notchforge_core::provider::ProviderEvent;
use notchforge_core::time::now_ms;
use objc2_app_kit::NSRunningApplication;
use objc2_foundation::NSString;
use std::cell::RefCell;
use std::ffi::c_void;
use tokio::sync::mpsc;

const NC_BUNDLE: &str = "com.apple.notificationcenterui";
const MAX_DEPTH: usize = 12;

struct Observer {
    observer: Owned,
    _app: Owned,
    source: CFTypeRef,
}

thread_local! {
    static STATE: RefCell<Option<Observer>> = const { RefCell::new(None) };
    static EVENTS: RefCell<Option<mpsc::Sender<ProviderEvent>>> = const { RefCell::new(None) };
}

fn nc_pid() -> Option<i32> {
    let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(NC_BUNDLE));
    apps.firstObject().map(|a| a.processIdentifier())
}

/// Copy an attribute (+1) of an AX element.
unsafe fn copy_attr(el: CFTypeRef, name: &str) -> Option<Owned> {
    let key = cf::cfstring(name)?;
    let mut out: CFTypeRef = std::ptr::null();
    let err = unsafe { AXUIElementCopyAttributeValue(el, key.as_ptr(), &mut out) };
    if err != kAXErrorSuccess { None } else { unsafe { Owned::from_create(out) } }
}

unsafe fn attr_string(el: CFTypeRef, name: &str) -> Option<String> {
    let v = unsafe { copy_attr(el, name) }?;
    unsafe { cf::as_string(v.as_ptr()) }
}

/// Visit an AX subtree depth-first.
unsafe fn walk(el: CFTypeRef, depth: usize, f: &mut dyn FnMut(CFTypeRef) -> bool) -> bool {
    if depth > MAX_DEPTH {
        return true;
    }
    if !f(el) {
        return false;
    }
    if let Some(children) = unsafe { copy_attr(el, "AXChildren") } {
        let n = unsafe { CFArrayGetCount(children.as_ptr()) };
        for i in 0..n {
            let child = unsafe { CFArrayGetValueAtIndex(children.as_ptr(), i) };
            if !unsafe { walk(child, depth + 1, f) } {
                return false;
            }
        }
    }
    true
}

/// Visible texts and descriptions inside an element.
unsafe fn texts(el: CFTypeRef) -> (Vec<String>, Vec<String>) {
    let mut texts = Vec::new();
    let mut descriptions = Vec::new();
    unsafe {
        walk(el, 0, &mut |e| {
            match attr_string(e, "AXRole").as_deref() {
                Some("AXStaticText") => {
                    if let Some(v) = attr_string(e, "AXValue").filter(|v| !v.trim().is_empty()) {
                        texts.push(v.trim().to_owned());
                    }
                }
                _ => {
                    if let Some(d) = attr_string(e, "AXDescription").filter(|d| !d.trim().is_empty()) {
                        descriptions.push(d);
                    }
                }
            }
            texts.len() < 16
        });
    }
    (texts, descriptions)
}

/// Turn banner texts into a notification (pure; unit-testable logic).
pub fn banner_to_notification(texts: &[String], descriptions: &[String]) -> Option<NewNotification> {
    let title = texts.first()?.clone();
    let body = texts[1..].join(" ");
    let all = format!("{} {}", descriptions.join(" "), texts.join(" "));
    let known = KnownApp::ALL
        .into_iter()
        .find(|k| descriptions.iter().any(|d| d.starts_with(k.display_name())))
        .or_else(|| KnownApp::from_web_text(&all));
    let app = match known {
        Some(k) => AppRef::known(k),
        None => AppRef { known: None, bundle_id: None, display_name: "Notification".into() },
    };
    let kind = match app.category() {
        notchforge_core::apps::AppCategory::Agent => notchforge_providers::usernoted::infer_agent_kind(&all),
        notchforge_core::apps::AppCategory::Messaging => NotificationKind::Message,
        _ => NotificationKind::Info,
    };
    Some(NewNotification {
        app,
        origin: Origin::AccessibilityBanner,
        kind,
        priority: None,
        title,
        subtitle: None,
        body,
        timestamp_ms: now_ms(),
        thread: None,
        url: None,
    })
}

/// `CFTypeRef` is a raw pointer; this wrapper lets a retained element ride a
/// main-queue dispatch (it is only ever touched on the main thread).
struct SendPtr(CFTypeRef);
// SAFETY: the pointer is retained and only dereferenced on the main thread.
unsafe impl Send for SendPtr {}

unsafe extern "C" fn on_ax_event(_obs: AXObserverRef, element: AXUIElementRef, _n: cf::CFStringRef, _refcon: *mut c_void) {
    // The banner's content is filled in just after the window appears.
    let retained = SendPtr(unsafe { CFRetain(element) });
    let when = dispatch2::DispatchTime::NOW.time(200_000_000);
    let _ = dispatch2::DispatchQueue::main().after(when, move || {
        let el = retained;
        // SAFETY: we own the +1 from CFRetain above.
        let owned = unsafe { Owned::from_create(el.0) };
        let Some(owned) = owned else { return };
        let (t, d) = unsafe { texts(owned.as_ptr()) };
        if let Some(n) = banner_to_notification(&t, &d) {
            EVENTS.with(|e| {
                if let Some(tx) = e.borrow().as_ref() {
                    let _ = tx.try_send(ProviderEvent::Notification(n));
                }
            });
        }
    });
}

/// Start observing banners. Must be called on the main thread.
pub fn start(events: mpsc::Sender<ProviderEvent>) -> Result<(), String> {
    stop();
    // SAFETY: no arguments.
    if unsafe { AXIsProcessTrusted() } == 0 {
        return Err("Accessibility permission not granted".into());
    }
    let pid = nc_pid().ok_or("Notification Center process not found")?;
    // SAFETY: standard AXObserver setup; created objects are owned (+1).
    unsafe {
        let app = Owned::from_create(AXUIElementCreateApplication(pid)).ok_or("AXUIElementCreateApplication failed")?;
        let mut obs: AXObserverRef = std::ptr::null();
        if AXObserverCreate(pid, on_ax_event, &mut obs) != kAXErrorSuccess {
            return Err("AXObserverCreate failed".into());
        }
        let observer = Owned::from_create(obs).ok_or("AXObserverCreate returned null")?;
        let name = cf::cfstring("AXWindowCreated").ok_or("CFString")?;
        if AXObserverAddNotification(observer.as_ptr(), app.as_ptr(), name.as_ptr(), std::ptr::null_mut()) != kAXErrorSuccess {
            return Err("could not observe Notification Center".into());
        }
        let source = AXObserverGetRunLoopSource(observer.as_ptr());
        CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopDefaultMode);
        STATE.with(|s| *s.borrow_mut() = Some(Observer { observer, _app: app, source }));
    }
    EVENTS.with(|e| *e.borrow_mut() = Some(events));
    tracing::info!(pid, "observing Notification Center banners via Accessibility");
    Ok(())
}

pub fn stop() {
    STATE.with(|s| {
        if let Some(o) = s.borrow_mut().take() {
            // SAFETY: removing the source we added; observer still alive here.
            unsafe { CFRunLoopRemoveSource(CFRunLoopGetMain(), o.source, kCFRunLoopDefaultMode) };
            drop(o.observer);
        }
    });
    EVENTS.with(|e| *e.borrow_mut() = None);
}

pub fn is_running() -> bool {
    STATE.with(|s| s.borrow().is_some())
}

/// Close the native banner showing `n`, using the banner's own "Close" action.
pub fn dismiss_banner(n: &Notification) {
    // SAFETY: AX calls on objects we own or borrow from owned arrays.
    unsafe {
        if AXIsProcessTrusted() == 0 {
            return;
        }
        let Some(pid) = nc_pid() else { return };
        let Some(app) = Owned::from_create(AXUIElementCreateApplication(pid)) else { return };
        let Some(windows) = copy_attr(app.as_ptr(), "AXWindows") else { return };
        for i in 0..CFArrayGetCount(windows.as_ptr()) {
            let win = CFArrayGetValueAtIndex(windows.as_ptr(), i);
            let (t, _) = texts(win);
            if !t.iter().any(|s| s == &n.title) {
                continue;
            }
            walk(win, 0, &mut |e| {
                let mut names: CFTypeRef = std::ptr::null();
                if AXUIElementCopyActionNames(e, &mut names) != kAXErrorSuccess {
                    return true;
                }
                let Some(names) = Owned::from_create(names) else { return true };
                for j in 0..CFArrayGetCount(names.as_ptr()) {
                    let action = CFArrayGetValueAtIndex(names.as_ptr(), j);
                    if cf::as_string(action).is_some_and(|a| a.contains("Close")) {
                        AXUIElementPerformAction(e, action);
                        return false;
                    }
                }
                true
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_mapping() {
        let n = banner_to_notification(&["Alice".into(), "hey".into(), "there".into()], &["Discord, Alice".into()])
            .expect("notification");
        assert_eq!(n.app.known, Some(KnownApp::Discord));
        assert_eq!(n.body, "hey there");
        assert!(banner_to_notification(&[], &[]).is_none());
    }
}
