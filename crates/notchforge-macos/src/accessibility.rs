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
use notchforge_core::geometry::Rect;
use notchforge_core::model::{NewNotification, Notification, NotificationKind, Origin};
use notchforge_core::provider::ProviderEvent;
use notchforge_core::time::now_ms;
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
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

/// PID of the process that renders banners. Looked up by bundle id, then by
/// executable name (it is not always registered as an `NSRunningApplication`).
fn nc_pid() -> Option<i32> {
    let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(NC_BUNDLE));
    if let Some(app) = apps.firstObject() {
        return Some(app.processIdentifier());
    }
    let out = std::process::Command::new("/usr/bin/pgrep").args(["-x", "NotificationCenter"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().next()?.trim().parse().ok()
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
        None => AppRef { known: None, bundle_id: None, display_name: "Notification".into(), platform: None },
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
        tokens: None,
    })
}

/// `CFTypeRef` is a raw pointer; this wrapper lets a retained element ride a
/// main-queue dispatch (it is only ever touched on the main thread).
struct SendPtr(CFTypeRef);
// SAFETY: the pointer is retained and only dereferenced on the main thread.
unsafe impl Send for SendPtr {}

unsafe extern "C" fn on_ax_event(
    _obs: AXObserverRef,
    element: AXUIElementRef,
    _n: cf::CFStringRef,
    _refcon: *mut c_void,
) {
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
        if AXObserverAddNotification(observer.as_ptr(), app.as_ptr(), name.as_ptr(), std::ptr::null_mut())
            != kAXErrorSuccess
        {
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

/// What a walk through one app's windows found.
#[derive(Debug, Default)]
pub struct CallWindow {
    /// A window offers both an answer and a decline button.
    pub ringing: bool,
    /// Both buttons are there, but nothing near them mentions a call.
    pub answer_and_decline: bool,
    /// A window offers a way to end a call, so one is in progress.
    pub hang_up: bool,
    pub texts: Vec<String>,
    /// What the window looks like, for diagnostics. Text only from small
    /// windows and from buttons about calls, so messages stay out of it.
    pub report: Vec<String>,
}

/// Elements a window may have and still count as small, like a call popup.
const SMALL_WINDOW: usize = 30;
const WALK_BUDGET: usize = 4000;
/// Web and Electron interfaces nest far deeper than native ones.
const DEEP_WALK: usize = 64;
/// How far up from a button to look for who is calling.
const CALLER_LEVELS: usize = 6;
/// How far up from a button wording about a call still counts.
const CALL_WORDING_LEVELS: usize = 5;

/// Chromium and Electron apps only build their accessibility tree when asked.
unsafe fn wake_web_accessibility(app: CFTypeRef) {
    for attribute in ["AXManualAccessibility", "AXEnhancedUserInterface"] {
        if let Some(key) = cf::cfstring(attribute) {
            unsafe { AXUIElementSetAttributeValue(app, key.as_ptr(), kCFBooleanTrue) };
        }
    }
}

/// Visit a subtree depth-first, last child first: a call's overlay is usually
/// added after the rest of a page, so this reaches it soonest.
unsafe fn walk_newest_first(el: CFTypeRef, depth: usize, f: &mut dyn FnMut(CFTypeRef) -> bool) -> bool {
    if depth > DEEP_WALK {
        return true;
    }
    if !f(el) {
        return false;
    }
    if let Some(children) = unsafe { copy_attr(el, "AXChildren") } {
        for i in (0..unsafe { CFArrayGetCount(children.as_ptr()) }).rev() {
            let child = unsafe { CFArrayGetValueAtIndex(children.as_ptr(), i) };
            if !unsafe { walk_newest_first(child, depth + 1, f) } {
                return false;
            }
        }
    }
    true
}

unsafe fn button_name(e: CFTypeRef) -> String {
    ["AXTitle", "AXDescription"]
        .iter()
        .filter_map(|a| unsafe { attr_string(e, a) })
        .find(|n| !n.trim().is_empty())
        .unwrap_or_default()
}

/// Could this role be a button? Web pages also use links and plain groups.
fn can_be_pressed(role: Option<&str>) -> bool {
    matches!(role, Some("AXButton" | "AXLink" | "AXGroup"))
}

/// Is this short and a control for a call? Only these are named from large
/// windows: a chat's rows are also buttons, and their names hold messages.
fn is_call_button(name: &str) -> bool {
    use notchforge_core::calls::{ANSWER_WORDS, DECLINE_WORDS, HANG_UP_WORDS, LEAVE_WORDS, action_matches};
    const CONTROLS: &[&str] = &["mute", "microphone", "camera", "speaker", "end ", "micro", "caméra", "terminer"];
    name.chars().count() <= 24
        && [ANSWER_WORDS, DECLINE_WORDS, HANG_UP_WORDS, LEAVE_WORDS, CONTROLS].iter().any(|w| action_matches(name, w))
}

/// The texts around a button: its nearest ancestor that says anything other
/// than the call's own wording, which is where the caller's name would be.
unsafe fn texts_around(button: CFTypeRef) -> Vec<String> {
    let mut current = unsafe { copy_attr(button, "AXParent") };
    for _ in 0..CALLER_LEVELS {
        let Some(el) = current else { break };
        if unsafe { attr_string(el.as_ptr(), "AXRole") }.as_deref() == Some("AXWindow") {
            break;
        }
        let (found, _) = unsafe { texts(el.as_ptr()) };
        if found.iter().any(|t| !notchforge_core::calls::is_call_wording(t)) {
            return found;
        }
        current = unsafe { copy_attr(el.as_ptr(), "AXParent") };
    }
    Vec::new()
}

/// Does anything around a button talk about a call?
unsafe fn call_mentioned_around(button: CFTypeRef) -> bool {
    use notchforge_core::calls::mentions_call;
    let mut current = unsafe { copy_attr(button, "AXParent") };
    // Close by only: higher up, a whole conversation's text comes into it.
    for _ in 0..CALL_WORDING_LEVELS {
        let Some(el) = current else { break };
        if unsafe { attr_string(el.as_ptr(), "AXRole") }.as_deref() == Some("AXWindow") {
            break;
        }
        let (found, _) = unsafe { texts(el.as_ptr()) };
        if found.iter().any(|t| mentions_call(t)) {
            return true;
        }
        current = unsafe { copy_attr(el.as_ptr(), "AXParent") };
    }
    false
}

/// How many windows the app with this pid has, if they can be read.
pub fn window_count(pid: i32) -> Option<usize> {
    // SAFETY: AX calls on objects we own or borrow from owned arrays.
    unsafe {
        if AXIsProcessTrusted() == 0 {
            return None;
        }
        let app = Owned::from_create(AXUIElementCreateApplication(pid))?;
        let windows = copy_attr(app.as_ptr(), "AXWindows")?;
        Some(CFArrayGetCount(windows.as_ptr()) as usize)
    }
}

/// The titles of an app's windows. A browser's show its current tab.
pub fn window_titles(pid: i32) -> Vec<String> {
    let mut out = Vec::new();
    // SAFETY: AX calls on objects we own or borrow from owned arrays.
    unsafe {
        if AXIsProcessTrusted() == 0 {
            return out;
        }
        let Some(app) = Owned::from_create(AXUIElementCreateApplication(pid)) else { return out };
        let Some(windows) = copy_attr(app.as_ptr(), "AXWindows") else { return out };
        for i in 0..CFArrayGetCount(windows.as_ptr()) {
            if let Some(t) = attr_string(CFArrayGetValueAtIndex(windows.as_ptr(), i), "AXTitle") {
                out.push(t);
            }
        }
    }
    out
}

/// Is the app with this pid showing answer and decline buttons?
pub fn inspect_call_window(pid: i32, web_app: bool) -> CallWindow {
    use notchforge_core::calls::{ANSWER_WORDS, DECLINE_WORDS, HANG_UP_WORDS, action_matches};
    let mut out = CallWindow::default();
    // SAFETY: AX calls on objects we own or borrow from owned arrays. An
    // element kept past the walk is retained.
    unsafe {
        if AXIsProcessTrusted() == 0 {
            out.report.push("Accessibility is not granted".to_owned());
            return out;
        }
        let Some(app) = Owned::from_create(AXUIElementCreateApplication(pid)) else { return out };
        if web_app {
            wake_web_accessibility(app.as_ptr());
        }
        let Some(windows) = copy_attr(app.as_ptr(), "AXWindows") else {
            out.report.push("no windows readable".to_owned());
            return out;
        };
        let count = CFArrayGetCount(windows.as_ptr());
        out.report.push(format!("{count} windows"));
        let mut budget = WALK_BUDGET;
        for i in 0..count {
            let win = CFArrayGetValueAtIndex(windows.as_ptr(), i);
            let (mut answer, mut decline, mut elements) = (false, false, 0usize);
            let mut buttons: Vec<String> = Vec::new();
            let mut call_buttons: Vec<String> = Vec::new();
            let mut window_texts: Vec<String> = Vec::new();
            let mut ringing_words: Vec<String> = Vec::new();
            let mut answer_el: Option<Owned> = None;
            walk_newest_first(win, 0, &mut |e| {
                elements += 1;
                budget = budget.saturating_sub(1);
                let role = attr_string(e, "AXRole");
                match role.as_deref() {
                    r if can_be_pressed(r) => {
                        let name = button_name(e);
                        // Long names are a chat's rows, which can mention anything.
                        let named_for_call = name.chars().count() <= 24;
                        if named_for_call && action_matches(&name, ANSWER_WORDS) && answer_el.is_none() {
                            answer = true;
                            answer_el = Owned::from_create(CFRetain(e));
                        }
                        decline |= named_for_call && action_matches(&name, DECLINE_WORDS);
                        if named_for_call && action_matches(&name, HANG_UP_WORDS) {
                            if !out.hang_up {
                                out.report.push(format!("hang-up button {name:?}"));
                            }
                            out.hang_up = true;
                        }
                        if r == Some("AXButton") && !name.is_empty() && buttons.len() < 12 {
                            buttons.push(name.clone());
                        }
                        if is_call_button(&name) && call_buttons.len() < 8 {
                            call_buttons.push(name);
                        }
                    }
                    Some("AXStaticText") => {
                        if let Some(v) = attr_string(e, "AXValue").filter(|v| !v.trim().is_empty()) {
                            if notchforge_core::calls::says_ringing_on_page(&v) && ringing_words.len() < 4 {
                                ringing_words.push(v.clone());
                            }
                            if elements <= SMALL_WINDOW {
                                window_texts.push(v);
                            }
                        }
                    }
                    _ => {}
                }
                budget > 0 && !(answer && decline)
            });
            let title = attr_string(win, "AXTitle").unwrap_or_default();
            let title_says_call = notchforge_core::calls::mentions_call(&title);
            // Some apps show no readable buttons, but their title says it all.
            if notchforge_core::calls::title_says_ringing(&title) && !(answer && decline) {
                out.ringing = true;
                out.report.push(format!("window {i} is ringing by its title, {elements} elements, no buttons"));
                out.texts.extend(window_texts);
                break;
            }
            let is_call = answer
                && decline
                && (title_says_call
                    || call_buttons.iter().any(|b| {
                        // Not a chat header's own "Video call" button.
                        notchforge_core::calls::mentions_call(b)
                            && (action_matches(b, ANSWER_WORDS) || action_matches(b, DECLINE_WORDS))
                    })
                    || answer_el.as_ref().is_some_and(|el| call_mentioned_around(el.as_ptr())));
            if answer && decline && !is_call {
                out.answer_and_decline = true;
                out.report.push(format!("window {i}: answer and decline, but nothing about a call"));
                continue;
            }
            if is_call {
                out.ringing = true;
                if title_says_call {
                    out.texts.push(title.clone());
                }
                if let Some(el) = &answer_el {
                    out.texts.extend(texts_around(el.as_ptr()));
                }
                out.texts.extend(window_texts);
                out.report.push(format!("window {i} is ringing: buttons {call_buttons:?} text {:?}", out.texts));
                break;
            }
            if elements <= SMALL_WINDOW {
                out.report.push(format!(
                    "window {i}: {elements} elements, title {title:?} buttons {buttons:?} text {window_texts:?}"
                ));
            } else {
                out.report.push(format!(
                    "window {i}: {elements} elements, call buttons {call_buttons:?}, ringing words {ringing_words:?}"
                ));
            }
            if budget == 0 {
                out.report.push("stopped early: too many elements".to_owned());
                break;
            }
        }
    }
    out
}

/// Press the button matching `words` in a window of the app with this pid.
pub fn press_window_button(pid: i32, words: &[&str]) -> bool {
    let mut pressed = false;
    // SAFETY: AX calls on objects we own or borrow from owned arrays.
    unsafe {
        if AXIsProcessTrusted() == 0 {
            return false;
        }
        let Some(app) = Owned::from_create(AXUIElementCreateApplication(pid)) else { return false };
        let Some(windows) = copy_attr(app.as_ptr(), "AXWindows") else { return false };
        let Some(press) = cf::cfstring("AXPress") else { return false };
        let mut budget = WALK_BUDGET;
        for i in 0..CFArrayGetCount(windows.as_ptr()) {
            let win = CFArrayGetValueAtIndex(windows.as_ptr(), i);
            walk_newest_first(win, 0, &mut |e| {
                budget = budget.saturating_sub(1);
                if !can_be_pressed(attr_string(e, "AXRole").as_deref()) {
                    return budget > 0;
                }
                let name = button_name(e);
                if name.chars().count() > 24 || !notchforge_core::calls::action_matches(&name, words) {
                    return budget > 0;
                }
                let result = AXUIElementPerformAction(e, press.as_ptr());
                pressed = result == kAXErrorSuccess || result == kAXErrorCannotComplete;
                !pressed && budget > 0
            });
            if pressed {
                break;
            }
        }
    }
    pressed
}

/// Open the native banner titled `title`, as a click on it would. A browser's
/// call notification answers this way ("Click to answer").
pub fn open_banner(title: &str) -> bool {
    let mut opened = false;
    // SAFETY: AX calls on objects we own or borrow from owned arrays.
    unsafe {
        if AXIsProcessTrusted() == 0 {
            return false;
        }
        let Some(pid) = nc_pid() else { return false };
        let Some(app) = Owned::from_create(AXUIElementCreateApplication(pid)) else { return false };
        let Some(windows) = copy_attr(app.as_ptr(), "AXWindows") else { return false };
        let Some(press) = cf::cfstring("AXPress") else { return false };
        for i in 0..CFArrayGetCount(windows.as_ptr()) {
            let win = CFArrayGetValueAtIndex(windows.as_ptr(), i);
            let (t, _) = texts(win);
            if !t.iter().any(|s| s == title) {
                continue;
            }
            walk(win, 0, &mut |e| {
                // The banner itself, not one of the buttons on it.
                if attr_string(e, "AXRole").as_deref() == Some("AXButton") {
                    return true;
                }
                let mut names: CFTypeRef = std::ptr::null();
                if AXUIElementCopyActionNames(e, &mut names) != kAXErrorSuccess {
                    return true;
                }
                let Some(names) = Owned::from_create(names) else { return true };
                for j in 0..CFArrayGetCount(names.as_ptr()) {
                    let action = CFArrayGetValueAtIndex(names.as_ptr(), j);
                    if cf::as_string(action).is_some_and(|n| n.contains("AXPress")) {
                        opened = AXUIElementPerformAction(e, press.as_ptr()) == kAXErrorSuccess;
                        return !opened;
                    }
                }
                true
            });
            if opened {
                break;
            }
        }
    }
    opened
}

/// Press the button or action on the native banner titled `title` whose name
/// matches `words`. Returns whether one was found and pressed.
pub fn press_banner_button(title: &str, words: &[&str]) -> bool {
    press_banner_button_in(nc_pid().unwrap_or(0), title, words)
}

/// Search one process's windows for a banner button matching `words`.
pub fn press_banner_button_in(pid: i32, title: &str, words: &[&str]) -> bool {
    let mut pressed = false;
    // SAFETY: AX calls on objects we own or borrow from owned arrays.
    unsafe {
        if AXIsProcessTrusted() == 0 || pid == 0 {
            return false;
        }
        let Some(app) = Owned::from_create(AXUIElementCreateApplication(pid)) else { return false };
        let Some(windows) = copy_attr(app.as_ptr(), "AXWindows") else { return false };
        for i in 0..CFArrayGetCount(windows.as_ptr()) {
            let win = CFArrayGetValueAtIndex(windows.as_ptr(), i);
            let (t, _) = texts(win);
            if !t.iter().any(|s| s == title) {
                continue;
            }
            walk(win, 0, &mut |e| {
                // A real button, named by its title or description.
                let named = ["AXTitle", "AXDescription"]
                    .iter()
                    .filter_map(|attr| attr_string(e, attr))
                    .any(|n| notchforge_core::calls::action_matches(&n, words));
                let role = attr_string(e, "AXRole").unwrap_or_default();
                let mut names: CFTypeRef = std::ptr::null();
                if AXUIElementCopyActionNames(e, &mut names) != kAXErrorSuccess {
                    return true;
                }
                let Some(names) = Owned::from_create(names) else { return true };
                for j in 0..CFArrayGetCount(names.as_ptr()) {
                    let action = CFArrayGetValueAtIndex(names.as_ptr(), j);
                    let Some(name) = cf::as_string(action) else { continue };
                    // Banners list their buttons as named custom actions.
                    let custom = notchforge_core::calls::action_matches(&name, words);
                    if custom || (named && role == "AXButton" && name.contains("AXPress")) {
                        AXUIElementPerformAction(e, action);
                        pressed = true;
                        return false;
                    }
                }
                true
            });
            if pressed {
                break;
            }
        }
    }
    pressed
}

// ── menu bar occupancy (wing dodging, notch placement) ──────────────────────

/// Is Accessibility granted? Wing dodging and notch placement read the menu
/// bar through it; without it nothing can be checked and the callers fail
/// safe (wings hidden, pill centred).
pub fn is_trusted() -> bool {
    // SAFETY: a plain AX call.
    unsafe { AXIsProcessTrusted() != 0 }
}

/// The menu bar item frames of the app owning `pid`'s menu, in AppKit
/// coordinates on the screen with the menu bar. The frontmost app's menu bar
/// covers the app menus left of the notch; the Control Center process owns the
/// system status items on its right.
pub fn menu_bar_item_rects(pid: i32, primary_h: f64) -> Vec<Rect> {
    bar_frames(pid, "AXMenuBar").into_iter().map(|f| to_appkit(f, primary_h)).collect()
}

/// An AX frame (top-left origin, like CG) in AppKit coordinates.
fn to_appkit(f: Rect, primary_h: f64) -> Rect {
    Rect::new(f.x, primary_h - f.y - f.height, f.width, f.height)
}

/// Frames, in AX coordinates, of the children of `pid`'s `bar` attribute
/// (`AXMenuBar` for app menus, `AXExtrasMenuBar` for status items). A slow
/// app is given up on after a short timeout.
fn bar_frames(pid: i32, bar: &str) -> Vec<Rect> {
    if pid == 0 || !is_trusted() || pid == std::process::id() as i32 {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: AX calls on objects we own or borrow from owned arrays.
    unsafe {
        let Some(app_el) = Owned::from_create(AXUIElementCreateApplication(pid)) else { return out };
        AXUIElementSetMessagingTimeout(app_el.as_ptr(), 0.1);
        let Some(menubar) = copy_attr(app_el.as_ptr(), bar) else { return out };
        let Some(items) = copy_attr(menubar.as_ptr(), "AXChildren") else { return out };
        for i in 0..CFArrayGetCount(items.as_ptr()) {
            let item = CFArrayGetValueAtIndex(items.as_ptr(), i);
            let (Some((x, y)), Some((w, h))) =
                (ax_value(item, "AXPosition", kAXValueCGPointType), ax_value(item, "AXSize", kAXValueCGSizeType))
            else {
                continue;
            };
            if w > 0.0 && h > 0.0 {
                out.push(Rect::new(x, y, w, h));
            }
        }
    }
    out
}

/// The frontmost app's menu items (the app menus left of the notch).
pub fn app_menu_rects(primary_h: f64) -> Vec<Rect> {
    match NSWorkspace::sharedWorkspace().frontmostApplication() {
        Some(app) => menu_bar_item_rects(app.processIdentifier(), primary_h),
        None => Vec::new(),
    }
}

/// Every app's status items, in AX coordinates, as last read by the scanner.
static EXTRAS: std::sync::Mutex<Vec<Rect>> = std::sync::Mutex::new(Vec::new());
static EXTRAS_SCANNER: std::sync::Once = std::sync::Once::new();

/// How often the scanner re-reads every app's status items.
const EXTRAS_EVERY: std::time::Duration = std::time::Duration::from_secs(3);

/// Read every running app's status items (`AXExtrasMenuBar`): current macOS
/// draws them inside the menu bar, so the window list can't see them. Asking
/// each app takes a while, so it runs on its own thread; a slow or hung app
/// only delays the next read, never the island.
fn scan_extras() {
    let mut last = usize::MAX;
    loop {
        if is_trusted() {
            // A pool per pass: this thread has no run loop to drain one.
            let pids: Vec<i32> = objc2::rc::autoreleasepool(|_| {
                NSWorkspace::sharedWorkspace().runningApplications().iter().map(|a| a.processIdentifier()).collect()
            });
            let mut found = Vec::new();
            for pid in pids {
                found.extend(bar_frames(pid, "AXExtrasMenuBar"));
            }
            if found.len() != last {
                last = found.len();
                tracing::info!(items = last, "menu bar status items read");
            }
            if let Ok(mut e) = EXTRAS.lock() {
                *e = found;
            }
        }
        std::thread::sleep(EXTRAS_EVERY);
    }
}

/// Status items in the menu bar: Control Center's (Wi-Fi, battery, clock),
/// read now, and every app's own, from the background scanner's last pass.
pub fn status_item_rects(primary_h: f64) -> Vec<Rect> {
    EXTRAS_SCANNER.call_once(|| {
        let _ = std::thread::Builder::new().name("menu-bar-extras".into()).spawn(scan_extras);
    });
    let cc = NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .find(|a| {
            let id = a.bundleIdentifier();
            id.as_deref().is_some_and(|s| s.to_string() == "com.apple.controlcenter")
        })
        .map(|a| a.processIdentifier())
        .unwrap_or(0);
    let mut out = menu_bar_item_rects(cc, primary_h);
    if let Ok(e) = EXTRAS.lock() {
        out.extend(e.iter().map(|f| to_appkit(*f, primary_h)));
    }
    out
}

/// Press the "Reply" button on the banner whose texts include `title`, which
/// expands the app's inline reply field (macOS keeps the banner on screen
/// while that field is open). `false` when there is no such banner or button.
pub fn press_banner_reply(title: &str) -> bool {
    press_banner_reply_in(None, title)
}

/// The same, but also searching an app's own banners first (web pushes from
/// Chrome-style browsers are windows of the browser process, not of
/// Notification Center). `bundle` narrows that search to the right browser.
pub fn press_banner_reply_in(bundle: Option<&str>, title: &str) -> bool {
    let words = notchforge_core::calls::REPLY_WORDS;
    if let Some(bundle) = bundle {
        for pid in running_pids(bundle) {
            if press_banner_button_in(pid, title, words) {
                return true;
            }
        }
    }
    press_banner_button(title, words)
}

/// Deliver a reply typed in the notch: find the banner with its reply field
/// already expanded (see [`press_banner_reply`]), type `text` into it, focus
/// it and confirm. The originating app sends the message through its own UI —
/// there is no public API to deliver a reply directly.
pub fn fill_banner_reply(title: &str, text: &str) -> bool {
    fill_banner_reply_in(None, title, text)
}

/// Deliver a reply through an app's own banner (web pushes). Falls back to
/// Notification Center like [`fill_banner_reply`].
pub fn fill_banner_reply_in(bundle: Option<&str>, title: &str, text: &str) -> bool {
    if let Some(bundle) = bundle {
        for pid in running_pids(bundle) {
            if fill_banner_reply_in_pid(pid, title, text) {
                return true;
            }
        }
    }
    fill_banner_reply_in_pid(nc_pid().unwrap_or(0), title, text)
}

/// The running pids of an app (by bundle id) — a browser delivering web
/// pushes owns its banners.
fn running_pids(bundle: &str) -> Vec<i32> {
    let apps =
        objc2_app_kit::NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle));
    apps.iter().map(|a| a.processIdentifier()).collect()
}

fn fill_banner_reply_in_pid(pid: i32, title: &str, text: &str) -> bool {
    let mut delivered = false;
    // SAFETY: AX calls on objects we own or borrow from owned arrays.
    unsafe {
        if AXIsProcessTrusted() == 0 || pid == 0 {
            return false;
        }
        let Some(app) = Owned::from_create(AXUIElementCreateApplication(pid)) else { return false };
        let Some(windows) = copy_attr(app.as_ptr(), "AXWindows") else { return false };
        for i in 0..CFArrayGetCount(windows.as_ptr()) {
            let win = CFArrayGetValueAtIndex(windows.as_ptr(), i);
            let (t, _) = texts(win);
            if !t.iter().any(|s| s == title) {
                continue;
            }
            // The reply field: a text area inside the matched banner.
            if walk_text_area(win, 0, text) {
                delivered = true;
                break;
            }
        }
    }
    delivered
}

/// unsafe: AX tree walk; sets the first unfilled text area's value, focuses
/// it and performs AXConfirm when the app offers it.
unsafe fn walk_text_area(el: CFTypeRef, depth: usize, text: &str) -> bool {
    // SAFETY: the whole walk touches only AX objects we own or borrow from
    // owned arrays, within this call's lifetime.
    unsafe {
        if depth > MAX_DEPTH {
            return false;
        }
        if attr_string(el, "AXRole").as_deref() == Some("AXTextArea") {
            let filled = attr_string(el, "AXValue").is_some_and(|v| !v.trim().is_empty());
            if !filled {
                let Some(value) = cf::cfstring(text) else { return false };
                let focused = cf::cfstring("AXFocused");
                if let Some(focused) = &focused {
                    AXUIElementSetAttributeValue(el, focused.as_ptr(), kCFBooleanTrue);
                }
                AXUIElementSetAttributeValue(el, value.as_ptr(), kCFBooleanTrue);
                // Some apps confirm the field directly; others need a Return key,
                // which the caller posts after focusing.
                let mut names: CFTypeRef = std::ptr::null();
                if AXUIElementCopyActionNames(el, &mut names) == kAXErrorSuccess {
                    if let Some(names) = Owned::from_create(names) {
                        for j in 0..CFArrayGetCount(names.as_ptr()) {
                            let action = CFArrayGetValueAtIndex(names.as_ptr(), j);
                            if cf::as_string(action).as_deref() == Some("AXConfirm") {
                                AXUIElementPerformAction(el, action);
                                break;
                            }
                        }
                    }
                }
                return true;
            }
        }
        if let Some(children) = copy_attr(el, "AXChildren") {
            for i in 0..CFArrayGetCount(children.as_ptr()) {
                let child = CFArrayGetValueAtIndex(children.as_ptr(), i);
                if walk_text_area(child, depth + 1, text) {
                    return true;
                }
            }
        }
        false
    }
}

/// An `AXValue` attribute (position or size) as two floats.
unsafe fn ax_value(el: CFTypeRef, name: &str, kind: u32) -> Option<(f64, f64)> {
    let v = unsafe { copy_attr(el, name) }?;
    let mut out = crate::ffi::cg::CGPoint { x: 0.0, y: 0.0 };
    // SAFETY: out is a CGPoint, the right backing store for both value kinds.
    let ok = unsafe { AXValueGetValue(v.as_ptr(), kind, &mut out as *mut crate::ffi::cg::CGPoint as *mut c_void) };
    (ok != 0).then_some((out.x, out.y))
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
