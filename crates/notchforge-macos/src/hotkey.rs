//! System-wide hotkeys via Carbon's `RegisterEventHotKey`, which needs no
//! Accessibility permission and only fires for the exact combinations:
//! * ⌃⌥N — show or hide the panel (the shortcut shown in the menu bar extra).
//! * ⌃⌥C — shelve the clipboard's text or link into the tray.

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
/// `kVK_ANSI_N`.
const KEY_N: u32 = 0x2D;
/// `kVK_ANSI_C`.
const KEY_C: u32 = 0x08;

unsafe extern "C" fn on_hot_key(_next: *mut c_void, _event: *mut c_void, user: *mut c_void) -> i32 {
    // Delivered on the main thread (application event target); `user` is the
    // hotkey id we registered with.
    match user as usize {
        1 => {
            crate::app::with_controller(|c| c.toggle());
        }
        2 => {
            crate::app::with_controller(|c| c.capture_clipboard());
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
        let status =
            RegisterEventHotKey(key_code, CONTROL_KEY | OPTION_KEY, id, GetApplicationEventTarget(), 0, &mut hotkey_ref);
        if status != 0 {
            tracing::warn!(status, "hotkey is taken by another app; it will not fire");
        }
    }
}

/// Register ⌃⌥N and ⌃⌥C.
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
}
