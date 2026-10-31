//! System-wide ⌃⌥N to show or hide the panel (the shortcut shown in the menu
//! bar extra). Uses Carbon's `RegisterEventHotKey`, which needs no
//! Accessibility permission and only fires for this exact combination.

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

unsafe extern "C" fn on_hot_key(_next: *mut c_void, _event: *mut c_void, _user: *mut c_void) -> i32 {
    // Delivered on the main thread (application event target).
    crate::app::with_controller(|c| c.toggle());
    0
}

/// Register ⌃⌥N. Failure (another app owns it) is logged, not fatal.
pub fn install() {
    let spec = EventTypeSpec { event_class: K_EVENT_CLASS_KEYBOARD, event_kind: K_EVENT_HOT_KEY_PRESSED };
    let mut handler_ref = std::ptr::null_mut();
    let mut hotkey_ref = std::ptr::null_mut();
    // SAFETY: plain Carbon calls with valid out-pointers; the handler is a
    // 'static fn and the registration lives for the process.
    unsafe {
        let target = GetApplicationEventTarget();
        let status = InstallEventHandler(target, on_hot_key, 1, &spec, std::ptr::null_mut(), &mut handler_ref);
        if status != 0 {
            tracing::warn!(status, "could not install the hotkey handler");
            return;
        }
        let id = EventHotKeyID { signature: u32::from_be_bytes(*b"NFrg"), id: 1 };
        let status = RegisterEventHotKey(KEY_N, CONTROL_KEY | OPTION_KEY, id, target, 0, &mut hotkey_ref);
        if status != 0 {
            tracing::warn!(status, "⌃⌥N is taken by another app; the panel shortcut is unavailable");
        }
    }
}
