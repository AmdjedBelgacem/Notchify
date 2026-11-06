//! Calls that ring in an app's own window or a browser tab.
//!
//! Most apps ring in a window of their own and send no notification. Any app
//! making sound is checked through Accessibility for an answer and a decline
//! button together, as is a known calling app that opens a new window. Browsers
//! only expose their current tab. Nothing is kept unless a call is found. The
//! checks run off the main thread, since large windows are slow to read.
//!
//! The same checks follow a call once it's picked up: its hang-up button or its
//! sound says it's still going.

use crate::app;
use dispatch2::DispatchQueue;
use notchforge_core::apps::is_browser;
use notchforge_core::calls::{
    ANSWER_WORDS, CALL_APP_BUNDLES, UNKNOWN_CALLER, browser_call_source, browser_source_name, call_app_name,
    call_tab_wanted, caller_from_texts, kind_from_text, never_rings,
};
use notchforge_core::engine::Command;
use notchforge_core::time::now_ms;
use objc2_app_kit::NSRunningApplication;
use objc2_foundation::NSString;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

/// How long to keep checking an app after it opens a window.
const NEW_WINDOW_WATCH_MS: i64 = 30_000;
/// Apps not known to place calls are checked this much less often.
const OTHER_APPS_EVERY: u32 = 3;

/// One check at a time: a slow app must not pile up threads.
static BUSY: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct WatchState {
    /// Window counts last seen, to notice a new one.
    windows: HashMap<i32, usize>,
    /// Apps to keep checking until this time, after opening a window.
    watch_until: HashMap<i32, i64>,
    /// The last report per pid, so diagnostics note changes rather than repeats.
    reports: HashMap<i32, String>,
    audio: String,
    ticks: u32,
}

thread_local! {
    static STATE: RefCell<WatchState> = RefCell::new(WatchState::default());
}

/// The process that owns an app's windows: sound often comes from a helper.
pub fn main_pid(bundle: &str, fallback: i32) -> i32 {
    let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle));
    apps.firstObject().map_or(fallback, |a| a.processIdentifier())
}

/// A calling app that is running: (pid, bundle id, name).
type Running = (i32, String, &'static str);

fn running_call_apps() -> Vec<Running> {
    let mut out = Vec::new();
    for bundle in CALL_APP_BUNDLES {
        let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle));
        for a in apps.iter() {
            if let Some(name) = call_app_name(bundle) {
                out.push((a.processIdentifier(), (*bundle).to_owned(), name));
            }
        }
    }
    out
}

/// Find a tab by its address or title, and bring it forward. The texts to look
/// for go in as arguments, since a tab's title is up to the site it's from.
fn tab_script(bundle: &str) -> Option<String> {
    let select = match bundle {
        "company.thebrowser.dia" => "focus t",
        "company.thebrowser.Browser" => "tell t to select",
        "com.google.Chrome" | "com.brave.Browser" | "com.microsoft.edgemac" => {
            "set active tab index of w to i\nset index of w to 1"
        }
        "com.apple.Safari" => "set current tab of w to t\nset index of w to 1",
        _ => return None,
    };
    let title = if bundle == "com.apple.Safari" { "name" } else { "title" };
    Some(format!(
        r#"on run argv
tell application id "{bundle}"
repeat with wanted in argv
repeat with w in windows
set i to 0
repeat with t in tabs of w
set i to i + 1
set here to ""
try
set here to (URL of t) & " " & ({title} of t)
end try
if here contains wanted then
{select}
activate
return "found"
end if
end repeat
end repeat
end repeat
end tell
return "none"
end run"#
    ))
}

/// Bring forward the tab or app a call is in, off the main thread: a browser
/// answering a script can take a while. Macs ask once before allowing this.
pub fn bring_forward(bundle: String, caller: String, source: String, url: Option<String>) {
    std::thread::spawn(move || {
        let found = tab_script(&bundle).is_some_and(|script| {
            std::process::Command::new("/usr/bin/osascript")
                .args(["-e", &script])
                .args(call_tab_wanted(&caller, &source))
                .output()
                .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "found")
        });
        tracing::info!(found, source = source.as_str(), "call diagnostics: bring forward");
        if !found {
            DispatchQueue::main().exec_async(move || {
                if !url.as_deref().is_some_and(crate::sharing::open_url) {
                    crate::sharing::open_app(&bundle);
                }
            });
        }
    });
}

pub fn tick() {
    let Some(snap) = app::with_controller(|c| c.current_snapshot()) else { return };
    if !app::with_config(|c| c.notifications.incoming_calls).unwrap_or(false) {
        return;
    }
    // Temporary, while detection is tuned against real apps: what is making sound.
    let audio = snap
        .audio
        .sources
        .iter()
        .map(|s| format!("{} ({}, pid {})", s.name, s.bundle_id.as_deref().unwrap_or("?"), s.pid))
        .collect::<Vec<_>>()
        .join(", ");
    STATE.with(|st| {
        if st.borrow().audio != audio {
            tracing::info!(%audio, "call diagnostics: apps making sound");
            st.borrow_mut().audio = audio;
        }
    });
    if BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    let now = now_ms();
    let running = running_call_apps();
    // Check apps making sound, apps already ringing, and apps that just opened a window.
    let mut targets: HashMap<i32, (String, String)> = HashMap::new();
    let mut browsers: Vec<(i32, String)> = Vec::new();
    let mut loud: HashSet<i32> = HashSet::new();
    let check_others = STATE.with(|st| {
        let mut st = st.borrow_mut();
        st.ticks = st.ticks.wrapping_add(1);
        st.ticks % OTHER_APPS_EVERY == 0
    });
    for s in &snap.audio.sources {
        let Some(bundle) = s.bundle_id.as_deref() else { continue };
        if never_rings(bundle) {
            continue;
        }
        loud.insert(main_pid(bundle, s.pid));
        if let Some(name) = call_app_name(bundle) {
            targets.insert(main_pid(bundle, s.pid), (bundle.to_owned(), name.to_owned()));
        } else if is_browser(bundle) {
            browsers.push((main_pid(bundle, s.pid), bundle.to_owned()));
        } else if check_others {
            targets.insert(main_pid(bundle, s.pid), (bundle.to_owned(), s.name.clone()));
        }
    }
    // A call in progress is followed, by its button and by its sound.
    if let Some(a) = &snap.active_call {
        if let Some(bundle) = a.app.bundle_id.as_deref() {
            let pid = a.window_pid.unwrap_or_else(|| main_pid(bundle, 0));
            if pid != 0 {
                targets.insert(pid, (bundle.to_owned(), a.source.clone()));
            }
            if snap.audio.sources.iter().any(|s| s.bundle_id.as_deref() == Some(bundle)) {
                app::send(Command::ActiveCallSound);
            }
        }
    }
    for c in &snap.calls {
        let Some(bundle) = c.app.bundle_id.as_deref() else { continue };
        // A call being answered may not have shown its window yet.
        let pid = c.window_pid.or_else(|| c.answering.then(|| main_pid(bundle, 0)).filter(|pid| *pid != 0));
        if let Some(pid) = pid {
            targets.insert(pid, (bundle.to_owned(), c.source.clone()));
        }
    }
    STATE.with(|st| {
        let mut st = st.borrow_mut();
        st.watch_until.retain(|_, until| *until > now);
        for (pid, bundle, name) in &running {
            if st.watch_until.contains_key(pid) {
                targets.insert(*pid, (bundle.clone(), (*name).to_owned()));
            }
        }
    });

    let known = STATE.with(|st| st.borrow().windows.clone());
    std::thread::spawn(move || {
        let counts: Vec<(i32, &'static str, Option<usize>)> =
            running.iter().map(|(pid, _, name)| (*pid, *name, crate::accessibility::window_count(*pid))).collect();
        // An app that just opened a window is checked now, not on the next round.
        for ((pid, bundle, name), (_, _, count)) in running.iter().zip(&counts) {
            if let (Some(count), Some(before)) = (count, known.get(pid)) {
                if count > before {
                    targets.insert(*pid, (bundle.clone(), (*name).to_owned()));
                }
            }
        }
        // A browser's current tab can ring from any site; known sites are named.
        for (pid, bundle) in browsers {
            let titles = crate::accessibility::window_titles(pid);
            match titles.iter().find_map(|t| browser_call_source(&bundle, t)) {
                Some(source) => {
                    targets.entry(pid).or_insert((bundle, source));
                }
                None if check_others => {
                    let source = browser_source_name(&bundle);
                    targets.entry(pid).or_insert((bundle, source));
                }
                None => {}
            }
        }
        let found: Vec<_> = targets
            .into_iter()
            .map(|(pid, (bundle, source))| {
                // Web interfaces only expose their buttons when asked, in known callers.
                let web = call_app_name(&bundle).is_some() || is_browser(&bundle);
                (pid, bundle, source, crate::accessibility::inspect_call_window(pid, web))
            })
            .collect();
        BUSY.store(false, Ordering::Release);
        DispatchQueue::main().exec_async(move || finish(counts, found, loud));
    });
}

/// Back on the main thread: act on what the check found.
fn finish(
    counts: Vec<(i32, &'static str, Option<usize>)>,
    found: Vec<(i32, String, String, crate::accessibility::CallWindow)>,
    loud: HashSet<i32>,
) {
    let now = now_ms();
    STATE.with(|st| {
        let mut st = st.borrow_mut();
        for (pid, name, count) in counts {
            let Some(count) = count else { continue };
            let before = st.windows.insert(pid, count);
            if before.is_some_and(|b| count > b) {
                tracing::info!(source = name, count, "call diagnostics: new window");
                st.watch_until.insert(pid, now + NEW_WINDOW_WATCH_MS);
            }
        }
        for (pid, _, source, window) in &found {
            let text = window.report.join(" ; ");
            if st.reports.insert(*pid, text.clone()).as_deref() != Some(text.as_str()) {
                tracing::info!(source = source.as_str(), %text, "call diagnostics: window");
            }
        }
    });
    let answering: Vec<_> = app::with_controller(|c| c.current_snapshot())
        .map(|snap| snap.calls.iter().filter(|c| c.answering).cloned().collect())
        .unwrap_or_default();
    for (pid, bundle, source, window) in found {
        let answering = answering.iter().find(|c| c.app.bundle_id.as_deref() == Some(bundle.as_str()));
        // A place that calls, making sound and showing both buttons, is ringing
        // even if the wording can't be read: some sites only say so in the title,
        // and only every other second.
        let calling_place = call_app_name(&bundle).is_some() || source != browser_source_name(&bundle);
        if !(window.ringing || (window.answer_and_decline && calling_place && loud.contains(&pid))) {
            if window.hang_up {
                match answering {
                    Some(c) => app::send(Command::CallAnsweredIn(c.id, pid)),
                    None => app::send(Command::WindowInCall { pid }),
                }
            }
            continue;
        }
        // Answer was pressed before its button could be: press it now.
        if let Some(c) = answering {
            if crate::accessibility::press_window_button(pid, ANSWER_WORDS) {
                tracing::info!(source = source.as_str(), "call diagnostics: answered once it appeared");
                app::send(Command::CallAnsweredIn(c.id, pid));
                continue;
            }
        }
        let all = window.texts.join(" ");
        app::send(Command::WindowCall {
            pid,
            bundle_id: bundle,
            source,
            caller: caller_from_texts(&window.texts, UNKNOWN_CALLER),
            kind: kind_from_text(&all),
        });
    }
}
