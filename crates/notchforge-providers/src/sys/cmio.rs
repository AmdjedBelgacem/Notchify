//! Camera-in-use detection (public CoreMediaIO API).

#![allow(non_upper_case_globals)]

//!
//! macOS draws its camera indicator pill beside the notch while a camera
//! runs. The pill is rendered inside the system menu bar surface — it is not
//! a window and not exposed as an Accessibility item — so the only way to
//! know is to ask the hardware whether a camera device is running somewhere.

use std::ffi::c_void;

pub type CMIOObjectID = u32;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CMIOObjectPropertyAddress {
    pub selector: u32,
    pub scope: u32,
    pub element: u32,
}

pub const kCMIOObjectSystemObject: CMIOObjectID = 1;
pub const kCMIOObjectPropertyScopeGlobal: u32 = fourcc(b"glob");
pub const kCMIOObjectPropertyElementMain: u32 = 0;
pub const kCMIOHardwarePropertyDevices: u32 = fourcc(b"dev#");
pub const kCMIODevicePropertyDeviceIsRunningSomewhere: u32 = fourcc(b"goin");

pub const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

#[link(name = "CoreMediaIO", kind = "framework")]
unsafe extern "C" {
    fn CMIOObjectGetPropertyDataSize(
        id: CMIOObjectID,
        address: *const CMIOObjectPropertyAddress,
        qualifiers_size: u32,
        qualifiers: *const c_void,
        data_size: *mut u32,
    ) -> i32;
    fn CMIOObjectGetPropertyData(
        id: CMIOObjectID,
        address: *const CMIOObjectPropertyAddress,
        qualifiers_size: u32,
        qualifiers: *const c_void,
        expected_size: u32,
        used_size: *mut u32,
        data: *mut c_void,
    ) -> i32;
}

fn get_array(selector: u32) -> Option<Vec<u32>> {
    let address = CMIOObjectPropertyAddress {
        selector,
        scope: kCMIOObjectPropertyScopeGlobal,
        element: kCMIOObjectPropertyElementMain,
    };
    // SAFETY: valid object/addresses; sizes checked before the read.
    unsafe {
        let mut size = 0u32;
        if CMIOObjectGetPropertyDataSize(kCMIOObjectSystemObject, &address, 0, std::ptr::null(), &mut size) != 0
            || size == 0
        {
            return None;
        }
        let mut buf = vec![0u32; size as usize / 4];
        let mut written = size;
        if CMIOObjectGetPropertyData(
            kCMIOObjectSystemObject,
            &address,
            0,
            std::ptr::null(),
            size,
            &mut written,
            buf.as_mut_ptr().cast(),
        ) != 0
        {
            return None;
        }
        buf.truncate(written as usize / 4);
        Some(buf)
    }
}

fn device_running(device: CMIOObjectID) -> bool {
    let address = CMIOObjectPropertyAddress {
        selector: kCMIODevicePropertyDeviceIsRunningSomewhere,
        scope: kCMIOObjectPropertyScopeGlobal,
        element: kCMIOObjectPropertyElementMain,
    };
    // SAFETY: valid device and property address; the value is a 4-byte
    // Boolean, sized by the size query before the read.
    unsafe {
        let mut size = 0u32;
        if CMIOObjectGetPropertyDataSize(device, &address, 0, std::ptr::null(), &mut size) != 0 || size == 0 {
            return false;
        }
        let mut running = 0u32;
        let mut written = size;
        if CMIOObjectGetPropertyData(
            device,
            &address,
            0,
            std::ptr::null(),
            size,
            &mut written,
            (&mut running as *mut u32).cast(),
        ) != 0
        {
            return false;
        }
        running != 0
    }
}

/// Is any camera running (in use by this or another app)?
pub fn camera_in_use() -> bool {
    let Some(devices) = get_array(kCMIOHardwarePropertyDevices) else { return false };
    devices.into_iter().any(device_running)
}
