//! Accessibility (AX) API from ApplicationServices.

#![allow(non_upper_case_globals)]

use super::cf::{CFArrayRef, CFDictionaryRef, CFStringRef, CFTypeRef};
use std::ffi::c_void;

pub type AXUIElementRef = CFTypeRef;
pub type AXObserverRef = CFTypeRef;
pub type AXValueRef = CFTypeRef;
pub type AXError = i32;
pub const kAXErrorSuccess: AXError = 0;
/// The app didn't reply, as when a button closes it before it can.
pub const kAXErrorCannotComplete: AXError = -25204;

/// `kAXValueCGPointType`.
pub const kAXValueCGPointType: u32 = 1;
/// `kAXValueCGSizeType`.
pub const kAXValueCGSizeType: u32 = 2;

pub type AXObserverCallback = unsafe extern "C" fn(
    observer: AXObserverRef,
    element: AXUIElementRef,
    notification: CFStringRef,
    refcon: *mut c_void,
);

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub static kAXTrustedCheckOptionPrompt: CFStringRef;
    pub fn AXIsProcessTrusted() -> u8;
    pub fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
    pub fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    pub fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    pub fn AXUIElementCopyActionNames(element: AXUIElementRef, names: *mut CFArrayRef) -> AXError;
    pub fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> AXError;
    pub fn AXUIElementSetAttributeValue(element: AXUIElementRef, attribute: CFStringRef, value: CFTypeRef) -> AXError;
    /// Reads an `AXValue` (position / size) into `out` (`CGPoint` / `CGSize`).
    pub fn AXValueGetValue(value: AXValueRef, the_type: u32, out: *mut c_void) -> u8;
    pub static kCFBooleanTrue: CFTypeRef;
    pub fn AXObserverCreate(application: i32, callback: AXObserverCallback, observer: *mut AXObserverRef) -> AXError;
    pub fn AXObserverAddNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
        refcon: *mut c_void,
    ) -> AXError;
    pub fn AXObserverGetRunLoopSource(observer: AXObserverRef) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub static kCFRunLoopDefaultMode: CFStringRef;
    pub fn CFRunLoopGetMain() -> CFTypeRef;
    pub fn CFRunLoopAddSource(rl: CFTypeRef, source: CFTypeRef, mode: CFStringRef);
    pub fn CFRunLoopRemoveSource(rl: CFTypeRef, source: CFTypeRef, mode: CFStringRef);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
}
