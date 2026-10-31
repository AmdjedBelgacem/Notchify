//! CoreGraphics display configuration (public API).

#![allow(non_upper_case_globals)]

use super::cf::{CFArrayRef, CFDictionaryRef, CFStringRef};
use std::ffi::c_void;

pub type CGDirectDisplayID = u32;
pub type CGDisplayModeRef = *const c_void;
pub type CGDisplayConfigRef = *mut c_void;
pub type CGError = i32;
/// Change lasts for the login session only (reverts on logout) — the safe option.
pub const kCGConfigureForSession: u32 = 1;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub static kCGDisplayShowDuplicateLowResolutionModes: CFStringRef;
    pub fn CGMainDisplayID() -> CGDirectDisplayID;
    pub fn CGGetActiveDisplayList(max: u32, displays: *mut CGDirectDisplayID, count: *mut u32) -> CGError;
    pub fn CGDisplayIsBuiltin(display: CGDirectDisplayID) -> i32;
    pub fn CGDisplayCopyAllDisplayModes(display: CGDirectDisplayID, options: CFDictionaryRef) -> CFArrayRef;
    pub fn CGDisplayCopyDisplayMode(display: CGDirectDisplayID) -> CGDisplayModeRef;
    pub fn CGDisplayModeGetWidth(mode: CGDisplayModeRef) -> usize;
    pub fn CGDisplayModeGetHeight(mode: CGDisplayModeRef) -> usize;
    pub fn CGDisplayModeGetPixelWidth(mode: CGDisplayModeRef) -> usize;
    pub fn CGDisplayModeGetPixelHeight(mode: CGDisplayModeRef) -> usize;
    pub fn CGDisplayModeGetRefreshRate(mode: CGDisplayModeRef) -> f64;
    pub fn CGDisplayModeIsUsableForDesktopGUI(mode: CGDisplayModeRef) -> bool;
    pub fn CGDisplayModeRelease(mode: CGDisplayModeRef);
    pub fn CGBeginDisplayConfiguration(config: *mut CGDisplayConfigRef) -> CGError;
    pub fn CGConfigureDisplayWithDisplayMode(
        config: CGDisplayConfigRef,
        display: CGDirectDisplayID,
        mode: CGDisplayModeRef,
        options: CFDictionaryRef,
    ) -> CGError;
    pub fn CGCompleteDisplayConfiguration(config: CGDisplayConfigRef, option: u32) -> CGError;
    pub fn CGCancelDisplayConfiguration(config: CGDisplayConfigRef) -> CGError;
}

// ── synthetic input (used only by the self-test) ──────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

pub type CGEventRef = *const c_void;
pub const kCGHIDEventTap: u32 = 0;
pub const kCGEventLeftMouseDown: u32 = 1;
pub const kCGEventLeftMouseUp: u32 = 2;
pub const kCGEventMouseMoved: u32 = 5;
pub const kCGMouseButtonLeft: u32 = 0;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub fn CGEventCreateMouseEvent(
        source: *const c_void,
        mouse_type: u32,
        position: CGPoint,
        button: u32,
    ) -> CGEventRef;
    pub fn CGEventCreateKeyboardEvent(source: *const c_void, keycode: u16, key_down: bool) -> CGEventRef;
    pub fn CGEventPost(tap: u32, event: CGEventRef);
    pub fn CGDisplayPixelsHigh(display: CGDirectDisplayID) -> usize;
}
