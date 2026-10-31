//! `NSScreen` → [`ScreenInfo`] conversion.

use notchforge_core::geometry::{Rect, ScreenInfo};
use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_foundation::{NSNumber, NSRect, NSString};

pub fn to_rect(r: NSRect) -> Rect {
    Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

pub fn to_nsrect(r: Rect) -> NSRect {
    crate::ui::rect(r.x, r.y, r.width, r.height)
}

/// `CGDirectDisplayID` of a screen (`deviceDescription["NSScreenNumber"]`).
pub fn display_id(screen: &NSScreen) -> u32 {
    let desc = screen.deviceDescription();
    let key = NSString::from_str("NSScreenNumber");
    desc.objectForKey(&key).and_then(|v| v.downcast::<NSNumber>().ok()).map_or(0, |n| n.unsignedIntValue())
}

fn aux_area(r: NSRect) -> Option<Rect> {
    (r.size.width > 0.0 && r.size.height > 0.0).then(|| to_rect(r))
}

pub fn screens(mtm: MainThreadMarker) -> Vec<ScreenInfo> {
    let main_id = NSScreen::mainScreen(mtm).map(|s| display_id(&s));
    NSScreen::screens(mtm)
        .iter()
        .map(|s| {
            let id = display_id(&s);
            let insets = s.safeAreaInsets();
            ScreenInfo {
                id,
                frame: to_rect(s.frame()),
                visible_frame: to_rect(s.visibleFrame()),
                safe_area_top: insets.top,
                aux_top_left: aux_area(s.auxiliaryTopLeftArea()),
                aux_top_right: aux_area(s.auxiliaryTopRightArea()),
                backing_scale: s.backingScaleFactor(),
                // SAFETY: plain display id.
                is_builtin: unsafe { crate::ffi::cg::CGDisplayIsBuiltin(id) } != 0,
                is_main: Some(id) == main_id,
            }
        })
        .collect()
}

/// Localised name for a display id (falls back to "Display <id>").
pub fn display_name(mtm: MainThreadMarker, id: u32) -> String {
    NSScreen::screens(mtm)
        .iter()
        .find(|s| display_id(s) == id)
        .map(|s| s.localizedName().to_string())
        .unwrap_or_else(|| format!("Display {id}"))
}
