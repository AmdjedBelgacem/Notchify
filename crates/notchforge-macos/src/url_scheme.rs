//! `notchisland://` links: drive the notch from Shortcuts, Raycast, Alfred,
//! a script (`open notchisland://tab/music`) or a web page.
//!
//! | Link | Does |
//! |---|---|
//! | `notchisland://open[?tab=music]` | Pin the panel open, on a tab |
//! | `notchisland://tab/<overview\|planner\|music\|live\|agents>` | Same, by path |
//! | `notchisland://toggle`, `notchisland://close` | Show / hide the panel |
//! | `notchisland://timer?minutes=25[&label=Focus]` | Start a timer |
//! | `notchisland://cancel-timer` | Cancel it |
//! | `notchisland://notify?title=…[&body=…]` | Show a notification |
//! | `notchisland://play-pause`, `next`, `previous` | Media keys |
//! | `notchisland://allow`, `notchisland://deny` | Answer the waiting tool request |
//! | `notchisland://shelve` | Shelve the clipboard into the tray |
//! | `notchisland://settings`, `notchisland://setup` | Open Settings / the setup |
//!
//! Links never carry data out: they can only act on the notch itself.

use crate::views::expanded::Tab;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};

/// What a link asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum UrlAction {
    Open(Option<Tab>),
    Toggle,
    Close,
    Timer { minutes: u32, label: String },
    CancelTimer,
    Notify { title: String, body: String },
    PlayPause,
    Next,
    Previous,
    Answer(bool),
    Shelve,
    Settings,
    Setup,
}

fn tab_named(name: &str) -> Option<Tab> {
    Some(match name.to_ascii_lowercase().as_str() {
        "overview" | "home" => Tab::Overview,
        "planner" | "calendar" | "notes" => Tab::Planner,
        "music" | "media" | "player" => Tab::Media,
        "live" | "sessions" => Tab::Live,
        "agents" | "usage" => Tab::Agents,
        _ => return None,
    })
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse a `notchisland://` link. Pure, so it is tested without AppKit.
pub fn parse(url: &str) -> Option<UrlAction> {
    let rest = url.strip_prefix("notchisland://").or_else(|| url.strip_prefix("notchisland:"))?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let q: Vec<(String, String)> = query
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (decode(k), decode(v))
        })
        .collect();
    let get = |k: &str| q.iter().find(|(qk, _)| qk == k).map(|(_, v)| v.clone());
    let mut parts = path.trim_matches('/').split('/');
    let head = parts.next().unwrap_or("").to_ascii_lowercase();
    Some(match head.as_str() {
        "" | "open" | "show" => UrlAction::Open(get("tab").as_deref().and_then(tab_named)),
        "tab" => UrlAction::Open(Some(tab_named(parts.next()?)?)),
        "toggle" => UrlAction::Toggle,
        "close" | "hide" => UrlAction::Close,
        "timer" => {
            let minutes = get("minutes").or_else(|| parts.next().map(str::to_owned))?.parse::<u32>().ok()?;
            if minutes == 0 || minutes > 24 * 60 {
                return None;
            }
            UrlAction::Timer { minutes, label: get("label").unwrap_or_else(|| format!("{minutes} min")) }
        }
        "cancel-timer" => UrlAction::CancelTimer,
        "notify" => {
            let title: String = get("title")?.chars().take(120).collect();
            if title.trim().is_empty() {
                return None;
            }
            UrlAction::Notify { title, body: get("body").unwrap_or_default().chars().take(500).collect() }
        }
        "play-pause" | "playpause" => UrlAction::PlayPause,
        "next" => UrlAction::Next,
        "previous" | "prev" => UrlAction::Previous,
        "allow" => UrlAction::Answer(true),
        "deny" => UrlAction::Answer(false),
        "shelve" => UrlAction::Shelve,
        "settings" => UrlAction::Settings,
        "setup" | "welcome" => UrlAction::Setup,
        _ => return None,
    })
}

/// Carry out a link's action on the main thread.
pub fn perform(action: UrlAction) {
    use crate::app::{send, with_controller};
    use notchforge_core::engine::Command;
    match action {
        UrlAction::Open(tab) => {
            with_controller(|c| {
                c.open_pinned();
                if let Some(t) = tab {
                    c.show_expanded_tab(t);
                }
            });
        }
        UrlAction::Toggle => {
            with_controller(|c| c.toggle());
        }
        UrlAction::Close => {
            with_controller(|c| c.collapse());
        }
        UrlAction::Timer { minutes, label } => {
            send(Command::StartTimer { duration_ms: minutes as i64 * 60_000, label });
        }
        UrlAction::CancelTimer => send(Command::CancelTimer),
        UrlAction::Notify { title, body } => {
            use notchforge_core::apps::AppRef;
            use notchforge_core::model::{NewNotification, NotificationKind, Origin, Priority};
            send(Command::Inject(Box::new(NewNotification {
                app: AppRef { known: None, bundle_id: None, display_name: "Shortcuts".into(), platform: None },
                origin: Origin::Internal,
                kind: NotificationKind::Info,
                priority: Some(Priority::Normal),
                title,
                subtitle: None,
                body,
                timestamp_ms: notchforge_core::time::now_ms(),
                thread: None,
                url: None,
                tokens: None,
            })));
        }
        UrlAction::PlayPause => drop(crate::media::press(crate::media::MediaKey::PlayPause)),
        UrlAction::Next => drop(crate::media::press(crate::media::MediaKey::Next)),
        UrlAction::Previous => drop(crate::media::press(crate::media::MediaKey::Previous)),
        UrlAction::Answer(allow) => {
            with_controller(|c| c.answer_pending_permission(allow));
        }
        UrlAction::Shelve => {
            with_controller(|c| c.capture_clipboard());
        }
        UrlAction::Settings => crate::app::show_settings(),
        UrlAction::Setup => crate::onboarding::show(),
    }
}

define_class!(
    /// Receives the `GURL` Apple Event macOS sends when a link is opened.
    // SAFETY: NSObject subclass with no ivars and no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    pub struct UrlHandler;

    impl UrlHandler {
        #[unsafe(method(handleURL:withReplyEvent:))]
        fn handle_url(&self, event: &AnyObject, _reply: &AnyObject) {
            // keyDirectObject ('----'): the link itself.
            let key = u32::from_be_bytes(*b"----");
            // SAFETY: NSAppleEventDescriptor API on the event AppKit passed us.
            let url: Option<String> = unsafe {
                let d: *mut AnyObject = msg_send![event, paramDescriptorForKeyword: key];
                if d.is_null() {
                    None
                } else {
                    let s: *mut objc2_foundation::NSString = msg_send![d, stringValue];
                    s.as_ref().map(|s| s.to_string())
                }
            };
            let Some(url) = url else { return };
            match parse(&url) {
                Some(action) => {
                    tracing::info!(?action, "link opened");
                    perform(action);
                }
                None => tracing::info!("unrecognised notchisland:// link"),
            }
        }
    }
);

thread_local! {
    static HANDLER: std::cell::RefCell<Option<Retained<UrlHandler>>> = const { std::cell::RefCell::new(None) };
}

/// Load the native Shortcuts actions (App Intents, built from
/// `packaging/intents/Intents.swift` into the bundle's Frameworks). They run
/// in this process and open the same links as above. Absent outside a bundle
/// built with Xcode, which is fine: links and the signed .shortcut files work.
fn load_shortcuts_actions() {
    let Some(lib) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("../Frameworks/libNotchislandIntents.dylib")))
        .filter(|p| p.exists())
    else {
        tracing::debug!("no native Shortcuts actions in this build");
        return;
    };
    let Ok(path) = std::ffi::CString::new(lib.to_string_lossy().as_bytes()) else { return };
    // SAFETY: a valid C path; the library only registers Swift types.
    let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW) };
    if handle.is_null() {
        // SAFETY: dlerror returns a static C string or null.
        let err = unsafe { libc::dlerror() };
        let why = if err.is_null() {
            String::new()
        } else {
            unsafe { std::ffi::CStr::from_ptr(err) }.to_string_lossy().into_owned()
        };
        tracing::warn!(error = %why, "could not load the Shortcuts actions");
    } else {
        tracing::info!("Shortcuts actions loaded");
    }
}

/// Start receiving `notchisland://` links (declared in Info.plist).
pub fn install(mtm: MainThreadMarker) {
    load_shortcuts_actions();
    let handler: Retained<UrlHandler> = unsafe { msg_send![UrlHandler::alloc(mtm), init] };
    let class_and_id = u32::from_be_bytes(*b"GURL"); // kInternetEventClass, kAEGetURL
    // SAFETY: NSAppleEventManager's documented registration; the handler is
    // kept alive for the process below.
    unsafe {
        let mgr: *mut AnyObject = msg_send![objc2::class!(NSAppleEventManager), sharedAppleEventManager];
        let _: () = msg_send![
            mgr,
            setEventHandler: &*handler,
            andSelector: sel!(handleURL:withReplyEvent:),
            forEventClass: class_and_id,
            andEventID: class_and_id
        ];
    }
    HANDLER.with(|h| *h.borrow_mut() = Some(handler));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_links() {
        assert_eq!(parse("notchisland://open"), Some(UrlAction::Open(None)));
        assert_eq!(parse("notchisland://open?tab=music"), Some(UrlAction::Open(Some(Tab::Media))));
        assert_eq!(parse("notchisland://tab/live"), Some(UrlAction::Open(Some(Tab::Live))));
        assert_eq!(parse("notchisland://tab/nope"), None);
        assert_eq!(
            parse("notchisland://timer?minutes=25&label=Deep%20work"),
            Some(UrlAction::Timer { minutes: 25, label: "Deep work".into() })
        );
        assert_eq!(parse("notchisland://timer/5"), Some(UrlAction::Timer { minutes: 5, label: "5 min".into() }));
        assert_eq!(parse("notchisland://timer?minutes=0"), None);
        assert_eq!(
            parse("notchisland://notify?title=Build+done&body=All+green"),
            Some(UrlAction::Notify { title: "Build done".into(), body: "All green".into() })
        );
        assert_eq!(parse("notchisland://notify?title=%20"), None);
        assert_eq!(parse("notchisland://allow"), Some(UrlAction::Answer(true)));
        assert_eq!(parse("notchisland://deny/"), Some(UrlAction::Answer(false)));
        assert_eq!(parse("https://example.com"), None);
        assert_eq!(parse("notchisland://rm-rf"), None);
    }
}
