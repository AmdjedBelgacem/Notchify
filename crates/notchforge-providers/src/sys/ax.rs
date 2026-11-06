//! Minimal Accessibility reads used by providers.
//!
//! Only one thing is needed today: the window titles of a running app, as the
//! fallback for browser tab titles when the browser has no usable scripting
//! dictionary (Firefox) or Automation was not granted. Every browser draws
//! its tab title into its window title, and Chromium browsers expose the AX
//! tree once woken with `AXManualAccessibility`.

use super::cf::{self, CFArrayGetCount, CFArrayGetValueAtIndex, CFTypeRef, Owned};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub fn AXIsProcessTrusted() -> u8;
    pub fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    pub fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: *const std::ffi::c_void,
        value: *mut CFTypeRef,
    ) -> i32;
}

/// The window titles of `pid`'s app, frontmost window first. Empty when
/// Accessibility is unavailable or the app exposes nothing.
pub fn window_titles(pid: i32) -> Vec<String> {
    // SAFETY: AX calls on objects we own or borrow from owned arrays.
    unsafe {
        if AXIsProcessTrusted() == 0 {
            return Vec::new();
        }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Vec::new();
        }
        let app = Owned::from_create(app);
        let Some(app) = app else { return Vec::new() };
        let mut windows: CFTypeRef = std::ptr::null();
        let key = match cf::cfstring("AXWindows") {
            Some(k) => k,
            None => return Vec::new(),
        };
        if AXUIElementCopyAttributeValue(app.as_ptr(), key.as_ptr(), &mut windows) != 0 {
            return Vec::new();
        }
        let Some(windows) = Owned::from_create(windows) else { return Vec::new() };
        let mut out = Vec::new();
        for i in 0..CFArrayGetCount(windows.as_ptr()) {
            let win = CFArrayGetValueAtIndex(windows.as_ptr(), i);
            let Some(title_key) = cf::cfstring("AXTitle") else { break };
            let mut title: CFTypeRef = std::ptr::null();
            if AXUIElementCopyAttributeValue(win, title_key.as_ptr(), &mut title) != 0 {
                continue;
            }
            let Some(t) = Owned::from_create(title).and_then(|t| cf::as_string(t.as_ptr())) else { continue };
            if !t.trim().is_empty() {
                out.push(t.trim().to_owned());
            }
            if out.len() >= 12 {
                break;
            }
        }
        out
    }
}
