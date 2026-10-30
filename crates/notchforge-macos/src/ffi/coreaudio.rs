//! CoreAudio hardware property API (public).

#![allow(non_upper_case_globals)]

use std::ffi::c_void;

pub type AudioObjectID = u32;
pub type OSStatus = i32;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioObjectPropertyAddress {
    pub selector: u32,
    pub scope: u32,
    pub element: u32,
}

pub const fn fourcc(s: &[u8; 4]) -> u32 {
    (s[0] as u32) << 24 | (s[1] as u32) << 16 | (s[2] as u32) << 8 | s[3] as u32
}

pub const kAudioObjectSystemObject: AudioObjectID = 1;
pub const kAudioHardwarePropertyDefaultOutputDevice: u32 = fourcc(b"dOut");
pub const kAudioObjectPropertyScopeGlobal: u32 = fourcc(b"glob");
pub const kAudioDevicePropertyScopeOutput: u32 = fourcc(b"outp");
pub const kAudioObjectPropertyElementMain: u32 = 0;
/// `kAudioHardwareServiceDeviceProperty_VirtualMainVolume` — the same scalar the
/// menu bar volume slider controls (works for devices without a hardware master).
pub const kVirtualMainVolume: u32 = fourcc(b"vmvc");
pub const kAudioDevicePropertyMute: u32 = fourcc(b"mute");

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    pub fn AudioObjectHasProperty(id: AudioObjectID, address: *const AudioObjectPropertyAddress) -> u8;
    pub fn AudioObjectGetPropertyData(
        id: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> OSStatus;
    pub fn AudioObjectSetPropertyData(
        id: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: u32,
        data: *const c_void,
    ) -> OSStatus;
}
