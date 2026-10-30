//! Minimal CoreFoundation FFI: just enough to walk the dictionaries returned by
//! IOKit's power-source API.

#![allow(non_upper_case_globals, non_snake_case)]

use std::ffi::{c_char, c_void};

pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFArrayRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFIndex = isize;
pub type CFTypeID = usize;

pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;
pub const kCFNumberSInt64Type: CFIndex = 4;
pub const kCFNumberFloat64Type: CFIndex = 6;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    pub fn CFStringGetTypeID() -> CFTypeID;
    pub fn CFNumberGetTypeID() -> CFTypeID;
    pub fn CFBooleanGetTypeID() -> CFTypeID;
    pub fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
    pub fn CFArrayGetValueAtIndex(array: CFArrayRef, idx: CFIndex) -> CFTypeRef;
    pub fn CFDictionaryGetValue(dict: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef;
    pub fn CFStringCreateWithBytes(
        alloc: CFTypeRef,
        bytes: *const u8,
        len: CFIndex,
        encoding: u32,
        is_external: u8,
    ) -> CFStringRef;
    pub fn CFStringGetLength(s: CFStringRef) -> CFIndex;
    pub fn CFStringGetMaximumSizeForEncoding(len: CFIndex, encoding: u32) -> CFIndex;
    pub fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: CFIndex, encoding: u32) -> u8;
    pub fn CFNumberGetValue(n: CFTypeRef, the_type: CFIndex, out: *mut c_void) -> u8;
    pub fn CFBooleanGetValue(b: CFTypeRef) -> u8;
}

/// An owned (+1 retained) CF object, released on drop.
pub struct Owned(CFTypeRef);

impl Owned {
    /// # Safety
    /// `ptr` must be a +1 retained CF object (from a `Create`/`Copy` function) or null.
    pub unsafe fn from_create(ptr: CFTypeRef) -> Option<Self> {
        if ptr.is_null() { None } else { Some(Self(ptr)) }
    }
    pub fn as_ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: we own exactly one retain count on a non-null object.
        unsafe { CFRelease(self.0) }
    }
}

pub fn cfstring(s: &str) -> Option<Owned> {
    // SAFETY: bytes/len describe a valid UTF-8 buffer; returns +1 or null.
    unsafe {
        Owned::from_create(CFStringCreateWithBytes(
            std::ptr::null(),
            s.as_ptr(),
            s.len() as CFIndex,
            kCFStringEncodingUTF8,
            0,
        ))
    }
}

/// Borrowed lookup of a string key. The result is valid while `dict` lives.
///
/// # Safety
/// `dict` must be a valid `CFDictionaryRef`.
pub unsafe fn dict_get(dict: CFDictionaryRef, key: &str) -> CFTypeRef {
    match cfstring(key) {
        Some(k) => unsafe { CFDictionaryGetValue(dict, k.as_ptr()) },
        None => std::ptr::null(),
    }
}

/// # Safety
/// `v` must be null or a valid CF object.
pub unsafe fn as_i64(v: CFTypeRef) -> Option<i64> {
    if v.is_null() || unsafe { CFGetTypeID(v) != CFNumberGetTypeID() } {
        return None;
    }
    let mut out: i64 = 0;
    let ok = unsafe { CFNumberGetValue(v, kCFNumberSInt64Type, (&mut out as *mut i64).cast()) };
    (ok != 0).then_some(out)
}

/// # Safety
/// `v` must be null or a valid CF object.
pub unsafe fn as_bool(v: CFTypeRef) -> Option<bool> {
    if v.is_null() || unsafe { CFGetTypeID(v) != CFBooleanGetTypeID() } {
        return None;
    }
    Some(unsafe { CFBooleanGetValue(v) } != 0)
}

/// # Safety
/// `v` must be null or a valid CF object.
pub unsafe fn as_string(v: CFTypeRef) -> Option<String> {
    if v.is_null() || unsafe { CFGetTypeID(v) != CFStringGetTypeID() } {
        return None;
    }
    let len = unsafe { CFStringGetLength(v) };
    let cap = unsafe { CFStringGetMaximumSizeForEncoding(len, kCFStringEncodingUTF8) } + 1;
    let mut buf = vec![0 as c_char; cap.max(1) as usize];
    let ok = unsafe { CFStringGetCString(v, buf.as_mut_ptr(), cap, kCFStringEncodingUTF8) };
    if ok == 0 {
        return None;
    }
    // SAFETY: CFStringGetCString NUL-terminated the buffer on success.
    let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Some(cstr.to_string_lossy().into_owned())
}
