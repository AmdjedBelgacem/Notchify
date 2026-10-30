//! Output volume of the default audio device (CoreAudio, public API).

use crate::ffi::coreaudio::*;
use std::ffi::c_void;

fn default_output_device() -> Option<AudioObjectID> {
    let addr = AudioObjectPropertyAddress {
        selector: kAudioHardwarePropertyDefaultOutputDevice,
        scope: kAudioObjectPropertyScopeGlobal,
        element: kAudioObjectPropertyElementMain,
    };
    let mut id: AudioObjectID = 0;
    let mut size = std::mem::size_of::<AudioObjectID>() as u32;
    // SAFETY: out buffer matches the property's data type and size.
    let status = unsafe {
        AudioObjectGetPropertyData(kAudioObjectSystemObject, &addr, 0, std::ptr::null(), &mut size, (&mut id as *mut u32).cast())
    };
    (status == 0 && id != 0).then_some(id)
}

fn output_addr(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { selector, scope: kAudioDevicePropertyScopeOutput, element: kAudioObjectPropertyElementMain }
}

/// Volume 0.0–1.0.
pub fn get() -> Option<f32> {
    let dev = default_output_device()?;
    let addr = output_addr(kVirtualMainVolume);
    // SAFETY: valid device id and address.
    if unsafe { AudioObjectHasProperty(dev, &addr) } == 0 {
        return None;
    }
    let mut v: f32 = 0.0;
    let mut size = std::mem::size_of::<f32>() as u32;
    // SAFETY: Float32 property.
    let status = unsafe { AudioObjectGetPropertyData(dev, &addr, 0, std::ptr::null(), &mut size, (&mut v as *mut f32).cast()) };
    (status == 0).then_some(v.clamp(0.0, 1.0))
}

pub fn set(value: f32) -> bool {
    let Some(dev) = default_output_device() else { return false };
    let addr = output_addr(kVirtualMainVolume);
    let v = value.clamp(0.0, 1.0);
    // SAFETY: Float32 property.
    let status = unsafe {
        AudioObjectSetPropertyData(dev, &addr, 0, std::ptr::null(), std::mem::size_of::<f32>() as u32, (&v as *const f32).cast::<c_void>())
    };
    if status == 0 && v > 0.0 {
        set_muted(false);
    }
    status == 0
}

pub fn muted() -> Option<bool> {
    let dev = default_output_device()?;
    let addr = output_addr(kAudioDevicePropertyMute);
    // SAFETY: valid device id and address.
    if unsafe { AudioObjectHasProperty(dev, &addr) } == 0 {
        return None;
    }
    let mut v: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: UInt32 property.
    let status = unsafe { AudioObjectGetPropertyData(dev, &addr, 0, std::ptr::null(), &mut size, (&mut v as *mut u32).cast()) };
    (status == 0).then_some(v != 0)
}

pub fn set_muted(muted: bool) -> bool {
    let Some(dev) = default_output_device() else { return false };
    let addr = output_addr(kAudioDevicePropertyMute);
    let v: u32 = muted.into();
    // SAFETY: UInt32 property.
    unsafe {
        AudioObjectSetPropertyData(dev, &addr, 0, std::ptr::null(), std::mem::size_of::<u32>() as u32, (&v as *const u32).cast::<c_void>()) == 0
    }
}
