//! AppKit side of audio & media.
//!
//! * Forwards the players' own change announcements (distributed
//!   notifications) and app launch/quit events to the media provider, so its
//!   poll can stay slow while the UI still updates instantly.
//! * Sends transport commands to the providers through the engine.
//! * Posts hardware media-key events for apps without a scripting interface
//!   (browsers, podcasts…). macOS routes them to the "Now Playing" app, exactly
//!   like the keyboard's ⏯ key. Posting events needs Accessibility.
//! * Checks / requests the Automation (Apple Events) permission per player.

use crate::app;
use block2::RcBlock;
use notchforge_core::engine::Command;
use notchforge_core::model::{MediaAction, MediaPlayer, Permission, PermissionState};
use notchforge_core::provider::ProviderCommand;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType, NSRunningApplication, NSWorkspace};
use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSOperationQueue, NSPoint, NSString};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

pub fn send(cmd: ProviderCommand) {
    app::send(Command::Provider(cmd));
}

pub fn control(player: MediaPlayer, action: MediaAction) {
    if player == MediaPlayer::Browser {
        // Browsers have no script interface: the system media keys reach them.
        match action {
            MediaAction::PlayPause => drop(press(MediaKey::PlayPause)),
            MediaAction::Next => drop(press(MediaKey::Next)),
            MediaAction::Previous => drop(press(MediaKey::Previous)),
            _ => {}
        }
        return;
    }
    send(ProviderCommand::Media { player, action });
}

pub fn set_output(uid: &str) {
    send(ProviderCommand::SetOutputDevice(uid.to_owned()));
}

/// Launch a player (used by the empty state's "Open Spotify" buttons).
pub fn open_player(player: MediaPlayer) {
    let ws = NSWorkspace::sharedWorkspace();
    if let Some(url) = ws.URLForApplicationWithBundleIdentifier(&NSString::from_str(player.bundle_id())) {
        ws.openURL(&url);
    }
}

pub fn is_installed(player: MediaPlayer) -> bool {
    NSWorkspace::sharedWorkspace()
        .URLForApplicationWithBundleIdentifier(&NSString::from_str(player.bundle_id()))
        .is_some()
}

/// Bring an app to the front by bundle id.
pub fn activate_app(bundle_id: &str) {
    let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id));
    if let Some(a) = apps.firstObject() {
        #[allow(deprecated)]
        a.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions::ActivateAllWindows);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Observers
// ─────────────────────────────────────────────────────────────────────────────

/// Keeps notification observers registered for the app's lifetime.
pub struct Observers {
    _tokens: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

const ANNOUNCEMENTS: [(&str, MediaPlayer); 3] = [
    ("com.spotify.client.PlaybackStateChanged", MediaPlayer::Spotify),
    ("com.apple.Music.playerInfo", MediaPlayer::Music),
    ("com.apple.iTunes.playerInfo", MediaPlayer::Music),
];

pub fn install_observers() -> Observers {
    let mut tokens = Vec::new();
    let main = NSOperationQueue::mainQueue();
    let center = NSDistributedNotificationCenter::defaultCenter();
    for (name, player) in ANNOUNCEMENTS {
        let block = RcBlock::new(move |_n: NonNull<NSNotification>| {
            send(ProviderCommand::MediaChanged(Some(player)));
        });
        // SAFETY: the block runs on the main queue, where `app::send` lives.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(&NSString::from_str(name)),
                None,
                Some(&main),
                &block,
            )
        };
        tokens.push(token);
    }

    let ws_center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: reading AppKit's notification-name constants.
    let names = unsafe {
        [
            objc2_app_kit::NSWorkspaceDidLaunchApplicationNotification,
            objc2_app_kit::NSWorkspaceDidTerminateApplicationNotification,
        ]
    };
    for name in names {
        let block = RcBlock::new(|n: NonNull<NSNotification>| {
            // SAFETY: AppKit passes a valid notification.
            let n = unsafe { n.as_ref() };
            let Some(info) = n.userInfo() else { return };
            // SAFETY: NSWorkspaceApplicationKey maps to an NSRunningApplication.
            let app: Option<Retained<NSRunningApplication>> =
                unsafe { msg_send![&*info, objectForKey: objc2_app_kit::NSWorkspaceApplicationKey] };
            let bundle = app.and_then(|a| a.bundleIdentifier()).map(|b| b.to_string());
            if let Some(player) = bundle.as_deref().and_then(MediaPlayer::from_bundle_id) {
                send(ProviderCommand::MediaChanged(Some(player)));
                send(ProviderCommand::ReloadPlaylists);
            }
        });
        // SAFETY: main-queue delivery, as above.
        let token =
            unsafe { ws_center.addObserverForName_object_queue_usingBlock(Some(name), None, Some(&main), &block) };
        tokens.push(token);
    }
    Observers { _tokens: tokens }
}

// ─────────────────────────────────────────────────────────────────────────────
// Media keys (any app)
// ─────────────────────────────────────────────────────────────────────────────

/// `NX_KEYTYPE_*` from IOKit's ev_keymap.h.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKey {
    PlayPause = 16,
    Next = 17,
    Previous = 18,
}

/// Post a media key press as if from the keyboard. Returns `false` when
/// Accessibility is missing (the system would silently drop the event); the
/// permission prompt is shown in that case.
pub fn press(key: MediaKey) -> bool {
    if crate::permissions::state(Permission::Accessibility) != PermissionState::Granted {
        crate::permissions::request(Permission::Accessibility);
        return false;
    }
    for down in [true, false] {
        let state: isize = if down { 0xa } else { 0xb };
        let data1 = ((key as isize) << 16) | (state << 8);
        let flags = NSEventModifierFlags::from_bits_retain(if down { 0xa00 } else { 0xb00 });
        // Subtype 8 = NX_SUBTYPE_AUX_CONTROL_BUTTONS.
        let ev = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::SystemDefined,
            NSPoint::new(0.0, 0.0),
            flags,
            0.0,
            0,
            None,
            8,
            data1,
            -1,
        );
        if let Some(cg) = ev.and_then(|e| e.CGEvent()) {
            objc2_core_graphics::CGEvent::post(objc2_core_graphics::CGEventTapLocation::HIDEventTap, Some(&cg));
        }
    }
    true
}

// ─────────────────────────────────────────────────────────────────────────────
// Automation permission (CoreServices)
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
struct AEDesc {
    descriptor_type: u32,
    data_handle: *mut std::ffi::c_void,
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn AECreateDesc(type_code: u32, data: *const std::ffi::c_void, size: isize, result: *mut AEDesc) -> i16;
    fn AEDisposeDesc(desc: *mut AEDesc) -> i16;
    fn AEDeterminePermissionToAutomateTarget(target: *const AEDesc, class: u32, id: u32, ask: u8) -> i32;
}

const TYPE_APPLICATION_BUNDLE_ID: u32 = u32::from_be_bytes(*b"bund");
const TYPE_WILDCARD: u32 = u32::from_be_bytes(*b"****");
const ERR_AE_EVENT_NOT_PERMITTED: i32 = -1743;
const ERR_AE_WOULD_REQUIRE_CONSENT: i32 = -1744;

/// Raw status of Automation for `player`. With `ask`, macOS shows its prompt
/// and the call **blocks** until the user answers, so never ask on the main
/// thread.
fn automation_status(player: MediaPlayer, ask: bool) -> i32 {
    let id = player.bundle_id();
    let mut desc = AEDesc { descriptor_type: 0, data_handle: std::ptr::null_mut() };
    // SAFETY: valid buffer/length; `desc` is disposed below.
    unsafe {
        if AECreateDesc(TYPE_APPLICATION_BUNDLE_ID, id.as_ptr().cast(), id.len() as isize, &mut desc) != 0 {
            return -1;
        }
        let status = AEDeterminePermissionToAutomateTarget(&desc, TYPE_WILDCARD, TYPE_WILDCARD, ask.into());
        AEDisposeDesc(&mut desc);
        status
    }
}

/// Combined Automation state for the running players, computed from scratch.
/// `AEDeterminePermissionToAutomateTarget` can block while a target app is
/// busy, so this only ever runs on a background thread.
fn compute_automation_state() -> PermissionState {
    let mut states = Vec::new();
    for p in MediaPlayer::ALL {
        match automation_status(p, false) {
            0 => states.push(PermissionState::Granted),
            ERR_AE_EVENT_NOT_PERMITTED => states.push(PermissionState::Denied),
            ERR_AE_WOULD_REQUIRE_CONSENT => states.push(PermissionState::NotDetermined),
            _ => {}
        }
    }
    if states.contains(&PermissionState::Denied) {
        PermissionState::Denied
    } else if states.contains(&PermissionState::NotDetermined) {
        PermissionState::NotDetermined
    } else if states.contains(&PermissionState::Granted) {
        PermissionState::Granted
    } else {
        PermissionState::Unknown
    }
}

static AUTOMATION: AtomicU8 = AtomicU8::new(0);
static AUTOMATION_CHECKING: AtomicBool = AtomicBool::new(false);

fn encode(s: PermissionState) -> u8 {
    match s {
        PermissionState::Unknown => 0,
        PermissionState::Granted => 1,
        PermissionState::Denied => 2,
        PermissionState::NotDetermined => 3,
        PermissionState::NotRequired => 4,
    }
}

fn decode(v: u8) -> PermissionState {
    match v {
        1 => PermissionState::Granted,
        2 => PermissionState::Denied,
        3 => PermissionState::NotDetermined,
        4 => PermissionState::NotRequired,
        _ => PermissionState::Unknown,
    }
}

/// Last known Automation state for the running players (`Unknown` until the
/// first check, or when neither player runs). Never blocks: each call
/// schedules a background re-check, so the next read is fresh.
pub fn automation_state() -> PermissionState {
    if !AUTOMATION_CHECKING.swap(true, Ordering::AcqRel) {
        std::thread::spawn(|| {
            AUTOMATION.store(encode(compute_automation_state()), Ordering::Release);
            AUTOMATION_CHECKING.store(false, Ordering::Release);
        });
    }
    decode(AUTOMATION.load(Ordering::Acquire))
}

/// Open Music, give it a moment to start, then ask for Automation so macOS
/// shows its consent prompt for it (the prompt needs the app running).
pub fn connect_music() {
    open_player(MediaPlayer::Music);
    std::thread::spawn(|| {
        for _ in 0..12 {
            std::thread::sleep(std::time::Duration::from_millis(700));
            if automation_status(MediaPlayer::Music, true) != -600 {
                break;
            }
        }
        dispatch2::DispatchQueue::main().exec_async(|| send(ProviderCommand::MediaChanged(None)));
    });
}

/// Ask for Automation for every running player (background thread), then
/// re-read the players. Opens System Settings when it was already refused.
pub fn request_automation() {
    if automation_state() == PermissionState::Denied {
        crate::permissions::open_settings(Permission::Automation);
        return;
    }
    std::thread::spawn(|| {
        for p in MediaPlayer::ALL {
            let _ = automation_status(p, true);
        }
        dispatch2::DispatchQueue::main().exec_async(|| send(ProviderCommand::MediaChanged(None)));
    });
}
