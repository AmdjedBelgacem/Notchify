//! What else is on screen, read from the public `CGWindowList` API.
//!
//! Window bounds, layer and owner PID need no permission (window *names*
//! would need Screen Recording; we never look at those). Two users:
//!
//! - **Fullscreen detection.** A fullscreen app owns a layer-0 window that
//!   spans the whole display, including the menu-bar strip. A window that is
//!   merely maximized stops at the visible frame (the menu bar is reserved),
//!   and a desktop with an auto-hidden menu bar has no full-height layer-0
//!   window at all — so both stay unflagged.
//! - **Wing dodging.** Status items are their own small windows one level
//!   above the menu bar; the menu bar itself is a single full-width window
//!   one level below, so matching the status level never hits it.

use crate::ffi::cf::{self, CFArrayGetCount, CFArrayGetValueAtIndex, CFDictionaryRef, CFTypeRef, Owned};
use crate::ffi::cg;
use notchforge_core::geometry::{Rect, ScreenInfo};
use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;

/// One on-screen window, in AppKit global coordinates (bottom-left origin).
#[derive(Debug, Clone, Copy)]
pub struct WindowInfo {
    pub layer: i32,
    pub pid: i32,
    pub bounds: Rect,
}

/// Every on-screen window. Must be called from the main thread (the coordinate
/// conversion needs the primary screen's height).
pub fn on_screen(mtm: MainThreadMarker) -> Vec<WindowInfo> {
    let primary_h = NSScreen::screens(mtm).iter().next().map_or(0.0, |s| s.frame().size.height);
    // SAFETY: CGWindowListCopyWindowInfo returns a retained array or null.
    let Some(list) = (unsafe {
        Owned::from_create(cg::CGWindowListCopyWindowInfo(cg::kCGWindowListOptionOnScreenOnly, cg::kCGNullWindowID))
    }) else {
        return Vec::new();
    };
    let count = unsafe { CFArrayGetCount(list.as_ptr()) };
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let dict = unsafe { CFArrayGetValueAtIndex(list.as_ptr(), i) as CFDictionaryRef };
        if dict.is_null() {
            continue;
        }
        let Some(layer) = int_value(dict, unsafe { cg::kCGWindowLayer }) else { continue };
        let pid = int_value(dict, unsafe { cg::kCGWindowOwnerPID }).unwrap_or(0);
        let mut cg_rect = cg::CGRect { origin: cg::CGPoint { x: 0.0, y: 0.0 }, size: cg::CGPoint { x: 0.0, y: 0.0 } };
        let bounds_ok =
            unsafe { cg::CGRectMakeWithDictionaryRepresentation(dict_value(dict, cg::kCGWindowBounds), &mut cg_rect) };
        if !bounds_ok {
            continue;
        }
        // CG is top-left origin, y down; AppKit is bottom-left, y up.
        let bounds =
            Rect::new(cg_rect.origin.x, primary_h - cg_rect.origin.y - cg_rect.size.y, cg_rect.size.x, cg_rect.size.y);
        out.push(WindowInfo { layer, pid, bounds });
    }
    out
}

fn dict_value(dict: CFDictionaryRef, key: CFTypeRef) -> CFDictionaryRef {
    // SAFETY: both are valid CF objects for the duration of the call.
    unsafe { cf::CFDictionaryGetValue(dict, key) }
}

fn int_value(dict: CFDictionaryRef, key: CFTypeRef) -> Option<i32> {
    let v = dict_value(dict, key);
    if v.is_null() {
        return None;
    }
    // SAFETY: v is the CFNumber stored under the key; kCFNumberIntType is 4.
    let mut out: i32 = 0;
    let ok = unsafe { cf::CFNumberGetValue(v, 4, &mut out as *mut i32 as *mut std::ffi::c_void) };
    (ok != 0).then_some(out)
}

/// Does a fullscreen app cover this display? See the module docs for why a
/// maximized window or an auto-hidden menu bar does not count.
pub fn fullscreen_covers(windows: &[WindowInfo], screen: &ScreenInfo) -> bool {
    let f = &screen.frame;
    windows.iter().any(|w| {
        w.layer == 0
            && w.bounds.x <= f.x + 1.0
            && w.bounds.y <= f.y + 1.0
            && w.bounds.max_x() >= f.max_x() - 1.0
            && w.bounds.max_y() >= f.max_y() - 1.0
            && is_regular_app(w.pid)
    })
}

/// The window's owner is a real, front-facing app. Background helpers —
/// automation overlays, capture daemons, menu-bar extras — can span the whole
/// screen at layer 0 without being fullscreen apps; activation policy tells
/// them apart (agents run as accessory/prohibited, real apps as regular).
fn is_regular_app(pid: i32) -> bool {
    let apps = objc2_app_kit::NSWorkspace::sharedWorkspace().runningApplications();
    apps.iter()
        .find(|a| a.processIdentifier() == pid)
        .is_some_and(|a| a.activationPolicy() == objc2_app_kit::NSApplicationActivationPolicy::Regular)
}

/// The frames of status items (Wi-Fi, battery, clock side, any menu bar
/// extra). Our own windows are excluded. Empty on current macOS, where status
/// items render inside the menu bar window — the Accessibility path sees them.
pub fn status_item_rects(windows: &[WindowInfo]) -> Vec<Rect> {
    let ours = std::process::id() as i32;
    windows
        .iter()
        .filter(|w| {
            w.layer == cg::kCGStatusWindowLevel
                && w.pid != ours
                // Status items are small; anything bigger is a menu or a panel.
                && w.bounds.width < 400.0
                && w.bounds.height < 60.0
        })
        .map(|w| w.bounds)
        .collect()
}
