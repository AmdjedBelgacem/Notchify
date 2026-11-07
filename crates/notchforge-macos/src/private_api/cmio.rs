//! Camera-in-use detection (public CoreMediaIO API).
//!
//! macOS draws its camera indicator pill immediately beside the notch, in the
//! same strip the island's right wing occupies. The pill is rendered inside
//! the system menu bar surface — it is not a window and not exposed as an
//! Accessibility item — so the only way to know it is there is to ask the
//! hardware whether the camera is running.

use crate::ffi::cmio::{
    CMIOObjectGetPropertyData, CMIOObjectGetPropertyDataSize, CMIOObjectID, CMIOObjectPropertyAddress,
    kCMIODevicePropertyDeviceIsRunningSomewhere, kCMIOHardwarePropertyDevices, kCMIOObjectPropertyElementMain,
    kCMIOObjectPropertyScopeGlobal, kCMIOObjectSystemObject,
};

fn get_array(selector: u32) -> Option<Vec<u32>> {
    let address = CMIOObjectPropertyAddress {
        selector,
        scope: kCMIOObjectPropertyScopeGlobal,
        element: kCMIOObjectPropertyElementMain,
    };
    // SAFETY: valid object/addresses; sizes checked before the read.
    unsafe {
        let mut size = 0usize;
        if CMIOObjectGetPropertyDataSize(kCMIOObjectSystemObject, &address, 0, std::ptr::null(), &mut size) != 0
            || size == 0
        {
            return None;
        }
        let mut buf = vec![0u32; size / 4];
        let mut written = size;
        if CMIOObjectGetPropertyData(
            kCMIOObjectSystemObject,
            &address,
            0,
            std::ptr::null(),
            written,
            &mut written,
            buf.as_mut_ptr() as _,
        ) != 0
        {
            return None;
        }
        buf.truncate(written / 4);
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
        let mut size = 0usize;
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
            &mut running as *mut u32 as _,
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
