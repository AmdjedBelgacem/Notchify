//! Display resolution switching (public CoreGraphics API).
//!
//! Safety net: changes are applied with `kCGConfigureForSession` (they never
//! outlive the login session) and the UI asks for confirmation, reverting
//! automatically after `controls.resolution_confirm_secs`.

use crate::ffi::cf::{self, CFArrayGetCount, CFArrayGetValueAtIndex, Owned};
use crate::ffi::cg::*;
use notchforge_core::model::{DisplayInfo, DisplayMode};
use objc2_foundation::{NSDictionary, NSNumber, NSString};

/// Stable identifier for a mode (CG's IO mode ids collide between HiDPI and
/// low-resolution variants, so we derive our own).
fn mode_id(w: usize, h: usize, pw: usize, ph: usize, hz: f64) -> i32 {
    let mut x: u64 = 1469598103934665603;
    for v in [w as u64, h as u64, pw as u64, ph as u64, (hz * 100.0).round() as u64] {
        x ^= v;
        x = x.wrapping_mul(1099511628211);
    }
    (x & 0x7fff_ffff) as i32
}

unsafe fn describe(mode: CGDisplayModeRef) -> DisplayMode {
    unsafe {
        let (w, h) = (CGDisplayModeGetWidth(mode), CGDisplayModeGetHeight(mode));
        let (pw, ph) = (CGDisplayModeGetPixelWidth(mode), CGDisplayModeGetPixelHeight(mode));
        let hz = CGDisplayModeGetRefreshRate(mode);
        DisplayMode { id: mode_id(w, h, pw, ph, hz), width: w as u32, height: h as u32, pixel_width: pw as u32, pixel_height: ph as u32, refresh_hz: hz }
    }
}

/// Copy all modes, including HiDPI duplicates. The returned array owns the modes.
fn copy_modes(display: CGDirectDisplayID) -> Option<Owned> {
    // SAFETY: the static key is a valid CFString; NSDictionary is toll-free
    // bridged to CFDictionary and outlives the call.
    unsafe {
        let key: &NSString = &*(kCGDisplayShowDuplicateLowResolutionModes as *const NSString);
        let yes = NSNumber::new_bool(true);
        let opts = NSDictionary::<NSString, NSNumber>::from_slices(&[key], &[&*yes]);
        let opts_ref = objc2::rc::Retained::as_ptr(&opts) as cf::CFDictionaryRef;
        Owned::from_create(CGDisplayCopyAllDisplayModes(display, opts_ref))
    }
}

pub fn active_displays() -> Vec<CGDirectDisplayID> {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    // SAFETY: buffer + out count.
    if unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) } != 0 {
        return Vec::new();
    }
    ids[..count as usize].to_vec()
}

/// Describe one display: current mode plus a curated list of usable modes.
pub fn display_info(display: CGDirectDisplayID, name: String) -> DisplayInfo {
    let mut modes: Vec<DisplayMode> = Vec::new();
    if let Some(arr) = copy_modes(display) {
        // SAFETY: array of CGDisplayModeRef borrowed from `arr`.
        unsafe {
            for i in 0..CFArrayGetCount(arr.as_ptr()) {
                let m = CFArrayGetValueAtIndex(arr.as_ptr(), i);
                if !CGDisplayModeIsUsableForDesktopGUI(m) {
                    continue;
                }
                let d = describe(m);
                if !modes.iter().any(|x| x.id == d.id) {
                    modes.push(d);
                }
            }
        }
    }
    // Prefer HiDPI variants and highest refresh for each logical size.
    modes.sort_by(|a, b| {
        b.width
            .cmp(&a.width)
            .then(b.height.cmp(&a.height))
            .then(b.is_hidpi().cmp(&a.is_hidpi()))
            .then(b.refresh_hz.total_cmp(&a.refresh_hz))
    });
    modes.dedup_by(|a, b| a.width == b.width && a.height == b.height && a.is_hidpi() == b.is_hidpi());
    modes.truncate(16);
    // SAFETY: Copy returns +1 retained mode (or null) which we release.
    let current = unsafe {
        let m = CGDisplayCopyDisplayMode(display);
        if m.is_null() {
            None
        } else {
            let d = describe(m);
            CGDisplayModeRelease(m);
            Some(d.id)
        }
    };
    // SAFETY: plain id.
    let is_builtin = unsafe { CGDisplayIsBuiltin(display) } != 0;
    DisplayInfo { id: display, name, is_builtin, current_mode_id: current, modes }
}

/// Apply a mode by our id for this session. Returns the previous mode id.
pub fn apply(display: CGDirectDisplayID, mode_id_wanted: i32) -> Result<Option<i32>, String> {
    let arr = copy_modes(display).ok_or("cannot list display modes")?;
    let previous = display_info(display, String::new()).current_mode_id;
    // SAFETY: modes borrowed from `arr` stay valid until it is dropped at the end.
    unsafe {
        for i in 0..CFArrayGetCount(arr.as_ptr()) {
            let m = CFArrayGetValueAtIndex(arr.as_ptr(), i);
            if describe(m).id != mode_id_wanted {
                continue;
            }
            let mut config: CGDisplayConfigRef = std::ptr::null_mut();
            if CGBeginDisplayConfiguration(&mut config) != 0 {
                return Err("CGBeginDisplayConfiguration failed".into());
            }
            if CGConfigureDisplayWithDisplayMode(config, display, m, std::ptr::null()) != 0 {
                CGCancelDisplayConfiguration(config);
                return Err("display rejected the mode".into());
            }
            if CGCompleteDisplayConfiguration(config, kCGConfigureForSession) != 0 {
                return Err("could not apply display mode".into());
            }
            return Ok(previous);
        }
    }
    Err("mode no longer available".into())
}
