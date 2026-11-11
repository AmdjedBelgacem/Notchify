//! System-wide hotkeys via Carbon's `RegisterEventHotKey`, which needs no
//! Accessibility permission and only fires for the exact combinations:
//! * ⌃⌥N — show or hide the panel (the shortcut shown in the menu bar extra).
//! * ⌃⌥C — shelve the clipboard's text or link into the tray.
//! * ⌃⌥Y / ⌃⌥D — allow / deny the coding agent's waiting tool request.
//! * ⌃⌥M — mute or unmute the microphone.
//! * ⌃⌥T — start a 5-minute timer.
//! * ⌃⌘← / ⌃⌘→ — move the island to the previous / next screen.

use std::ffi::c_void;

#[repr(C)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

type EventHandlerProc = unsafe extern "C" fn(next: *mut c_void, event: *mut c_void, user: *mut c_void) -> i32;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: EventHandlerProc,
        num_types: u32,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut *mut c_void,
    ) -> i32;
    fn GetEventParameter(
        event: *mut c_void,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buffer_size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> i32;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: *mut c_void,
        options: u32,
        out_ref: *mut *mut c_void,
    ) -> i32;
}

const K_EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
const CONTROL_KEY: u32 = 1 << 12;
const OPTION_KEY: u32 = 1 << 11;
const COMMAND_KEY: u32 = 1 << 8;
/// `kVK_LeftArrow` / `kVK_RightArrow`.
const KEY_LEFT: u32 = 123;
const KEY_RIGHT: u32 = 124;
/// `kVK_ANSI_N`.
const KEY_N: u32 = 0x2D;
/// `kVK_ANSI_C`.
const KEY_C: u32 = 0x08;
/// `kVK_ANSI_Y`.
const KEY_Y: u32 = 0x10;
/// `kVK_ANSI_D`.
const KEY_D: u32 = 0x02;
/// `kVK_ANSI_M`.
const KEY_M: u32 = 0x2E;
/// `kVK_ANSI_T`.
const KEY_T: u32 = 0x11;

/// `kEventParamDirectObject` and `typeEventHotKeyID`.
const PARAM_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const TYPE_HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");

unsafe extern "C" fn on_hot_key(_next: *mut c_void, event: *mut c_void, _user: *mut c_void) -> i32 {
    // Delivered on the main thread (application event target). Which hotkey
    // fired is in the event's direct object: the id we registered it with.
    let mut hk = EventHotKeyID { signature: 0, id: 0 };
    // SAFETY: `hk` is a valid, correctly sized out-buffer for the parameter.
    let status = unsafe {
        GetEventParameter(
            event,
            PARAM_DIRECT_OBJECT,
            TYPE_HOT_KEY_ID,
            std::ptr::null_mut(),
            std::mem::size_of::<EventHotKeyID>(),
            std::ptr::null_mut(),
            (&mut hk as *mut EventHotKeyID).cast(),
        )
    };
    if status != 0 {
        return 0;
    }
    let user = hk.id as usize;
    match user {
        1 => {
            crate::app::with_controller(|c| c.toggle());
        }
        2 => {
            crate::app::with_controller(|c| c.capture_clipboard());
        }
        3 | 4 => {
            crate::app::with_controller(|c| c.answer_pending_permission(user == 3));
        }
        5 => {
            crate::app::with_controller(|c| c.toggle_mic());
        }
        6 => {
            crate::app::send(notchforge_core::engine::Command::StartTimer {
                duration_ms: 5 * 60_000,
                label: "5 min".into(),
            });
        }
        7 | 8 => {
            let next = user == 8;
            crate::app::with_controller(|c| c.focus_neighbour_screen(next));
        }
        _ => {}
    }
    0
}

/// Register one combination. Failure (another app owns it) is logged, not fatal.
fn register(key_code: u32, id: usize) {
    let mut hotkey_ref = std::ptr::null_mut();
    // SAFETY: plain Carbon call with a valid out-pointer; the registration
    // lives for the process and hands `id` back to the handler.
    unsafe {
        let id = EventHotKeyID { signature: u32::from_be_bytes(*b"NFrg"), id: id as u32 };
        let status = RegisterEventHotKey(
            key_code,
            CONTROL_KEY | OPTION_KEY,
            id,
            GetApplicationEventTarget(),
            0,
            &mut hotkey_ref,
        );
        if status != 0 {
            tracing::warn!(status, "hotkey is taken by another app; it will not fire");
        }
    }
}

/// Register ⌃⌥N, ⌃⌥C, ⌃⌥Y and ⌃⌥D.
pub fn install() {
    let spec = EventTypeSpec { event_class: K_EVENT_CLASS_KEYBOARD, event_kind: K_EVENT_HOT_KEY_PRESSED };
    let mut handler_ref = std::ptr::null_mut();
    // SAFETY: plain Carbon calls with valid out-pointers; the handler is a
    // 'static fn and the registration lives for the process.
    unsafe {
        let target = GetApplicationEventTarget();
        let status = InstallEventHandler(target, on_hot_key, 1, &spec, std::ptr::null_mut(), &mut handler_ref);
        if status != 0 {
            tracing::warn!(status, "could not install the hotkey handler");
            return;
        }
    }
    register(KEY_N, 1);
    register(KEY_C, 2);
    register(KEY_Y, 3);
    register(KEY_D, 4);
    register(KEY_M, 5);
    register(KEY_T, 6);
    for (key, id) in [(KEY_LEFT, 7u32), (KEY_RIGHT, 8)] {
        let mut hotkey_ref = std::ptr::null_mut();
        // SAFETY: plain Carbon call with a valid out-pointer.
        unsafe {
            let id = EventHotKeyID { signature: u32::from_be_bytes(*b"NFrg"), id };
            let status = RegisterEventHotKey(
                key,
                COMMAND_KEY | CONTROL_KEY,
                id,
                GetApplicationEventTarget(),
                0,
                &mut hotkey_ref,
            );
            if status != 0 {
                tracing::warn!(status, "screen-cycle hotkey is taken by another app; it will not fire");
            }
        }
    }
}
