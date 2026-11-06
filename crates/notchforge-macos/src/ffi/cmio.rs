//! CoreMediaIO hardware API (public): camera devices and their run state.
//! Used to know when the system camera indicator occupies the menu bar
//! beside the notch.

#![allow(non_upper_case_globals)]

use super::cf::CFTypeRef;
use std::ffi::c_void;

pub type CMIOObjectID = u32;
pub type OSStatus = i32;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CMIOObjectPropertyAddress {
    pub selector: u32,
    pub scope: u32,
    pub element: u32,
}

#[link(name = "CoreMediaIO", kind = "framework")]
unsafe extern "C" {
    pub fn CMIOObjectGetPropertyDataSize(
        object: CMIOObjectID,
        address: *const CMIOObjectPropertyAddress,
        qualifiers_size: u32,
        qualifiers: *const c_void,
        size: *mut usize,
    ) -> OSStatus;
    pub fn CMIOObjectGetPropertyData(
        object: CMIOObjectID,
        address: *const CMIOObjectPropertyAddress,
        qualifiers_size: u32,
        qualifiers: *const c_void,
        expected_size: usize,
        size: *mut usize,
        data: *mut c_void,
    ) -> OSStatus;
}

pub const kCMIOObjectSystemObject: CMIOObjectID = 1;
pub const kCMIOObjectPropertyScopeGlobal: u32 = fourcc(b"glob");
pub const kCMIOObjectPropertyElementMain: u32 = 0;
pub const kCMIOHardwarePropertyDevices: u32 = fourcc(b"dev#");
pub const kCMIODevicePropertyDeviceIsRunningSomewhere: u32 = fourcc(b"goin");

pub const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

// silence unused import when features change
#[allow(dead_code)]
const _: CFTypeRef = std::ptr::null();
