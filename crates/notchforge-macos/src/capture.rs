//! Quick capture from the notch: screenshots of every kind, screen recording
//! and voice memos.
//!
//! Screenshots and screen recordings go through the system's own
//! `screencapture` tool, so they look and behave exactly like ⌘⇧3/4/5 (and use
//! the Screen Recording permission macOS asks for on first use). Voice memos
//! record the microphone with AVFoundation's `AVAudioRecorder`. Every finished
//! file lands on the Shelf, ready to drag.

use notchforge_core::engine::Command;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadOnly, msg_send};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use std::path::PathBuf;
use std::process::Child;
use std::sync::Mutex;

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

/// A screenshot, by what it captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shot {
    /// Drag out an area (⌘⇧4).
    Selection,
    /// Click a window (⌘⇧4, then Space).
    Window,
    /// The whole screen (⌘⇧3).
    Screen,
    /// The whole screen after a 5-second countdown.
    Timed,
    /// An area, straight to the clipboard.
    SelectionToClipboard,
    /// The system capture toolbar (⌘⇧5): every option, recording included.
    Toolbar,
}

/// What is being recorded right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recording {
    Screen,
    Voice,
}

struct Active {
    kind: Recording,
    started_ms: i64,
    path: PathBuf,
    /// The `screencapture -v` process (screen recording).
    child: Option<Child>,
    /// The `AVAudioRecorder` (voice memo).
    recorder: Option<Retained<AnyObject>>,
}

// SAFETY: the recorder is only created, used and released on the main thread
// (every entry point below asserts it); the mutex only guards the slot.
unsafe impl Send for Active {}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

/// The recording in progress and when it started (epoch ms).
pub fn active() -> Option<(Recording, i64)> {
    ACTIVE.lock().ok()?.as_ref().map(|a| (a.kind, a.started_ms))
}

fn stamp() -> String {
    let out = std::process::Command::new("/bin/date").arg("+%Y-%m-%d at %H.%M.%S").output();
    out.ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_else(|| "now".into())
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Where a capture is saved: screenshots where macOS saves them (the Desktop
/// unless changed), recordings in Movies, voice memos in Music.
fn target(kind: &str, ext: &str) -> PathBuf {
    let dir = match kind {
        "Screen Recording" => home().join("Movies"),
        "Voice Memo" => home().join("Music").join("Notchisland Voice Memos"),
        _ => screenshot_dir(),
    };
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("{kind} {}.{ext}", stamp()))
}

fn screenshot_dir() -> PathBuf {
    let out =
        std::process::Command::new("/usr/bin/defaults").args(["read", "com.apple.screencapture", "location"]).output();
    out.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .map(|s| PathBuf::from(s.replacen('~', &home().to_string_lossy(), 1)))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| home().join("Desktop"))
}

/// Put a finished capture on the Shelf.
fn shelve(path: PathBuf) {
    if path.is_file() {
        crate::app::send(Command::TrayAdd(vec![path]));
    }
}

/// Take a screenshot. The island closes first so it isn't in the picture;
/// interactive kinds wait for the selection like ⌘⇧4 does.
pub fn screenshot(shot: Shot) {
    crate::app::with_controller(|c| c.collapse());
    let path = target("Screenshot", "png");
    let mut args: Vec<String> = Vec::new();
    match shot {
        Shot::Selection => args.extend(["-i", "-s"].map(String::from)),
        Shot::Window => args.extend(["-i", "-w", "-o"].map(String::from)),
        Shot::Screen => {}
        Shot::Timed => args.extend(["-T", "5"].map(String::from)),
        Shot::SelectionToClipboard => args.extend(["-i", "-s", "-c"].map(String::from)),
        Shot::Toolbar => args.extend(["-i", "-U"].map(String::from)),
    }
    let to_file = !matches!(shot, Shot::SelectionToClipboard);
    if to_file {
        args.push(path.to_string_lossy().into_owned());
    }
    std::thread::spawn(move || {
        // A beat for the island to fold away before the screen is captured.
        std::thread::sleep(std::time::Duration::from_millis(250));
        let status = std::process::Command::new("/usr/sbin/screencapture").args(&args).status();
        if let Err(e) = status {
            tracing::warn!(error = %e, "screencapture failed");
            return;
        }
        if to_file {
            dispatch2::DispatchQueue::main().exec_async(move || shelve(path));
        }
    });
}

/// Start recording the whole screen (stop with [`stop`]). macOS asks for the
/// Screen Recording permission the first time.
pub fn start_screen_recording() {
    assert!(objc2::MainThreadMarker::new().is_some());
    if active().is_some() {
        return;
    }
    crate::app::with_controller(|c| c.collapse());
    let path = target("Screen Recording", "mov");
    // -v: record video until interrupted; -C: include the pointer; -k: show clicks.
    match std::process::Command::new("/usr/sbin/screencapture").args(["-v", "-C", "-k"]).arg(&path).spawn() {
        Ok(child) => {
            tracing::info!("screen recording started");
            *ACTIVE.lock().expect("capture lock") = Some(Active {
                kind: Recording::Screen,
                started_ms: notchforge_core::time::now_ms(),
                path,
                child: Some(child),
                recorder: None,
            });
            changed();
        }
        Err(e) => tracing::warn!(error = %e, "could not start screen recording"),
    }
}

/// Start a voice memo from the microphone (stop with [`stop`]). macOS asks
/// for the microphone the first time.
pub fn start_voice_memo() {
    assert!(objc2::MainThreadMarker::new().is_some());
    if active().is_some() {
        return;
    }
    let path = target("Voice Memo", "m4a");
    let Some(recorder) = audio_recorder(&path) else {
        crate::app::alert(
            "Voice memo",
            "The microphone could not be opened. Allow Notchisland under Microphone in System Settings › Privacy & Security.",
        );
        return;
    };
    // SAFETY: a valid AVAudioRecorder.
    let ok: bool = unsafe { msg_send![&*recorder, record] };
    if !ok {
        crate::app::alert(
            "Voice memo",
            "Recording could not start. Allow Notchisland under Microphone in System Settings › Privacy & Security.",
        );
        return;
    }
    tracing::info!("voice memo started");
    *ACTIVE.lock().expect("capture lock") = Some(Active {
        kind: Recording::Voice,
        started_ms: notchforge_core::time::now_ms(),
        path,
        child: None,
        recorder: Some(recorder),
    });
    changed();
}

/// An `AVAudioRecorder` writing AAC (mono, 44.1 kHz) to `path`.
fn audio_recorder(path: &std::path::Path) -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"AVAudioRecorder")?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    // The AVFoundation key constants are their own names.
    let keys = [
        NSString::from_str("AVFormatIDKey"),
        NSString::from_str("AVSampleRateKey"),
        NSString::from_str("AVNumberOfChannelsKey"),
    ];
    let values = [NSNumber::new_u32(u32::from_be_bytes(*b"aac ")), NSNumber::new_f64(44_100.0), NSNumber::new_u32(1)];
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let value_refs: Vec<&AnyObject> = values.iter().map(|v| -> &AnyObject { v.as_ref() }).collect();
    let settings = NSDictionary::<NSString, AnyObject>::from_slices(&key_refs, &value_refs);
    // SAFETY: documented initializer; a nil error pointer is allowed.
    unsafe {
        let alloc: *mut AnyObject = msg_send![cls, alloc];
        let rec: *mut AnyObject =
            msg_send![alloc, initWithURL: &*url, settings: &*settings, error: std::ptr::null_mut::<*mut AnyObject>()];
        let rec = Retained::from_raw(rec)?;
        let _: bool = msg_send![&*rec, prepareToRecord];
        Some(rec)
    }
}

/// Stop whatever is recording and put the file on the Shelf.
pub fn stop() {
    let Some(mut a) = ACTIVE.lock().ok().and_then(|mut s| s.take()) else { return };
    if let Some(rec) = a.recorder.take() {
        // SAFETY: a valid AVAudioRecorder.
        let _: () = unsafe { msg_send![&*rec, stop] };
        shelve(a.path.clone());
    }
    if let Some(mut child) = a.child.take() {
        // screencapture -v finishes and writes the movie on an interrupt.
        // SAFETY: signalling our own child process.
        unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
        let path = a.path.clone();
        std::thread::spawn(move || {
            let _ = child.wait();
            dispatch2::DispatchQueue::main().exec_async(move || shelve(path));
        });
    }
    tracing::info!(kind = ?a.kind, "recording stopped");
    changed();
}

/// Recording started or stopped: the shoulders show it, and tick with it.
fn changed() {
    crate::app::with_controller(|c| c.capture_changed());
}

thread_local! {
    /// The open capture menu's item targets (kept alive while it shows).
    static MENU_TARGETS: std::cell::RefCell<Vec<Retained<crate::ui::ActionTarget>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The capture menu, at the pointer: screenshots of every kind, screen
/// recording and voice memos (or Stop while one records).
pub fn show_menu() {
    use objc2_app_kit::{NSEvent, NSMenu, NSMenuItem};
    let Some(mtm) = objc2::MainThreadMarker::new() else { return };
    let menu = NSMenu::new(mtm);
    let mut targets = Vec::new();
    let mut add = |menu: &NSMenu, title: &str, symbol: &str, f: Box<dyn Fn()>| {
        let t = crate::ui::ActionTarget::new(mtm, move |_| f());
        // SAFETY: a valid selector; the target is kept alive in MENU_TARGETS.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &crate::ui::ns(&crate::i18n::tr(title)),
                Some(crate::ui::ActionTarget::action()),
                &crate::ui::ns(""),
            )
        };
        // SAFETY: as above.
        unsafe { item.setTarget(Some(t.as_any())) };
        item.setImage(crate::ui::symbol_image(symbol, 13.0, "").as_deref());
        menu.addItem(&item);
        targets.push(t);
    };
    let header = |menu: &NSMenu, title: &str| {
        menu.addItem(&NSMenuItem::sectionHeaderWithTitle(&crate::ui::ns(&crate::i18n::tr(title)), mtm));
    };
    match active() {
        Some((Recording::Screen, _)) => {
            header(&menu, "Recording the screen");
            add(&menu, "Stop recording", "stop.circle", Box::new(stop));
        }
        Some((Recording::Voice, _)) => {
            header(&menu, "Recording a voice memo");
            add(&menu, "Stop recording", "stop.circle", Box::new(stop));
        }
        None => {
            header(&menu, "Screenshot");
            add(&menu, "Selection", "rectangle.dashed", Box::new(|| screenshot(Shot::Selection)));
            add(&menu, "Window", "macwindow", Box::new(|| screenshot(Shot::Window)));
            add(&menu, "Whole screen", "display", Box::new(|| screenshot(Shot::Screen)));
            add(&menu, "Whole screen in 5 seconds", "timer", Box::new(|| screenshot(Shot::Timed)));
            add(
                &menu,
                "Selection to clipboard",
                "doc.on.clipboard",
                Box::new(|| screenshot(Shot::SelectionToClipboard)),
            );
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            header(&menu, "Record");
            add(&menu, "Record the screen", "record.circle", Box::new(start_screen_recording));
            add(&menu, "Record a voice memo", "mic", Box::new(start_voice_memo));
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            add(&menu, "All capture options…", "camera.viewfinder", Box::new(|| screenshot(Shot::Toolbar)));
        }
    }
    MENU_TARGETS.with(|m| *m.borrow_mut() = targets);
    menu.popUpMenuPositioningItem_atLocation_inView(None, NSEvent::mouseLocation(), None);
}
