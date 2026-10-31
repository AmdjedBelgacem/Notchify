//! Screen brightness of the built-in display.

use crate::ffi::cg;
use crate::private_api::display_services;

/// The built-in panel if present, else the main display.
pub fn target_display() -> u32 {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    // SAFETY: buffer of 16 ids, count out-pointer.
    let ok = unsafe { cg::CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) } == 0;
    if ok {
        for &id in &ids[..count as usize] {
            // SAFETY: id comes from the active display list.
            if unsafe { cg::CGDisplayIsBuiltin(id) } != 0 {
                return id;
            }
        }
    }
    // SAFETY: no arguments.
    unsafe { cg::CGMainDisplayID() }
}

pub fn get() -> Option<f32> {
    display_services::get(target_display())
}

pub fn set(value: f32) -> bool {
    display_services::set(target_display(), value)
}

pub fn is_supported() -> bool {
    display_services::is_available() && get().is_some()
}
