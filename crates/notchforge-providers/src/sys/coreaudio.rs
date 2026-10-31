//! CoreAudio hardware API (public): output devices, their capabilities, the
//! default output, and — on macOS 14.2+ — which processes are producing audio.
//!
//! Everything goes through `AudioObjectGetPropertyData`; the helpers below keep
//! the unsafe surface to a handful of generic functions.

#![allow(non_upper_case_globals)]

use super::cf;
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

/// `AudioObjectPropertyListenerProc`.
pub type ListenerProc = unsafe extern "C" fn(
    id: AudioObjectID,
    count: u32,
    addresses: *const AudioObjectPropertyAddress,
    client_data: *mut c_void,
) -> OSStatus;

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    pub fn AudioObjectHasProperty(id: AudioObjectID, address: *const AudioObjectPropertyAddress) -> u8;
    pub fn AudioObjectGetPropertyDataSize(
        id: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
    ) -> OSStatus;
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
    pub fn AudioObjectAddPropertyListener(
        id: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        listener: ListenerProc,
        client_data: *mut c_void,
    ) -> OSStatus;
    pub fn AudioObjectRemovePropertyListener(
        id: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        listener: ListenerProc,
        client_data: *mut c_void,
    ) -> OSStatus;
}

pub const fn fourcc(s: &[u8; 4]) -> u32 {
    (s[0] as u32) << 24 | (s[1] as u32) << 16 | (s[2] as u32) << 8 | s[3] as u32
}

pub const kAudioObjectSystemObject: AudioObjectID = 1;
pub const kAudioObjectPropertyScopeGlobal: u32 = fourcc(b"glob");
pub const kAudioDevicePropertyScopeOutput: u32 = fourcc(b"outp");
pub const kAudioObjectPropertyElementMain: u32 = 0;

pub const kAudioHardwarePropertyDevices: u32 = fourcc(b"dev#");
pub const kAudioHardwarePropertyDefaultOutputDevice: u32 = fourcc(b"dOut");
pub const kAudioHardwarePropertyDefaultSystemOutputDevice: u32 = fourcc(b"sOut");
/// macOS 14.2+.
pub const kAudioHardwarePropertyProcessObjectList: u32 = fourcc(b"prs#");

pub const kAudioObjectPropertyName: u32 = fourcc(b"lnam");
pub const kAudioDevicePropertyDeviceUID: u32 = fourcc(b"uid ");
pub const kAudioDevicePropertyTransportType: u32 = fourcc(b"tran");
pub const kAudioDevicePropertyNominalSampleRate: u32 = fourcc(b"nsrt");
pub const kAudioDevicePropertyAvailableNominalSampleRates: u32 = fourcc(b"nsr#");
pub const kAudioDevicePropertyStreamConfiguration: u32 = fourcc(b"slay");
pub const kAudioDevicePropertyStreams: u32 = fourcc(b"stm#");
pub const kAudioStreamPropertyPhysicalFormat: u32 = fourcc(b"pft ");
pub const kAudioDevicePropertyIsHidden: u32 = fourcc(b"hidn");
pub const kAudioDevicePropertyDeviceCanBeDefaultDevice: u32 = fourcc(b"dflt");
pub const kAudioDevicePropertyDeviceCanBeDefaultSystemDevice: u32 = fourcc(b"sflt");
/// `kAudioHardwareServiceDeviceProperty_VirtualMainVolume`.
pub const kVirtualMainVolume: u32 = fourcc(b"vmvc");
pub const kAudioDevicePropertyMute: u32 = fourcc(b"mute");

pub const kAudioProcessPropertyPID: u32 = fourcc(b"ppid");
pub const kAudioProcessPropertyBundleID: u32 = fourcc(b"pbid");
pub const kAudioProcessPropertyDevices: u32 = fourcc(b"pdv#");
pub const kAudioProcessPropertyIsRunningOutput: u32 = fourcc(b"piro");

pub const fn addr(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { selector, scope, element: kAudioObjectPropertyElementMain }
}

pub const fn global(selector: u32) -> AudioObjectPropertyAddress {
    addr(selector, kAudioObjectPropertyScopeGlobal)
}

pub const fn output(selector: u32) -> AudioObjectPropertyAddress {
    addr(selector, kAudioDevicePropertyScopeOutput)
}

// ─────────────────────────────────────────────────────────────────────────────
// Generic property access
// ─────────────────────────────────────────────────────────────────────────────

pub fn has(id: AudioObjectID, a: AudioObjectPropertyAddress) -> bool {
    // SAFETY: plain query with a valid address struct.
    unsafe { AudioObjectHasProperty(id, &a) != 0 }
}

/// Read a fixed-size property. `T` must be the property's exact C type.
pub fn get<T: Copy + Default>(id: AudioObjectID, a: AudioObjectPropertyAddress) -> Option<T> {
    if !has(id, a) {
        return None;
    }
    let mut v = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: the buffer is exactly `size` bytes of the property's type.
    let status =
        unsafe { AudioObjectGetPropertyData(id, &a, 0, std::ptr::null(), &mut size, (&mut v as *mut T).cast()) };
    (status == 0 && size as usize == std::mem::size_of::<T>()).then_some(v)
}

/// Read a variable-length array property of `T` elements.
pub fn get_array<T: Copy + Default>(id: AudioObjectID, a: AudioObjectPropertyAddress) -> Option<Vec<T>> {
    if !has(id, a) {
        return None;
    }
    let mut size = 0u32;
    // SAFETY: size query only.
    if unsafe { AudioObjectGetPropertyDataSize(id, &a, 0, std::ptr::null(), &mut size) } != 0 {
        return None;
    }
    let elem = std::mem::size_of::<T>().max(1);
    // Round up so the buffer always covers `size` bytes.
    let n = (size as usize).div_ceil(elem);
    let mut v = vec![T::default(); n];
    if n == 0 {
        return Some(v);
    }
    size = (n * elem) as u32;
    // SAFETY: the buffer holds `size` bytes of `T`s.
    let status = unsafe { AudioObjectGetPropertyData(id, &a, 0, std::ptr::null(), &mut size, v.as_mut_ptr().cast()) };
    if status != 0 {
        return None;
    }
    v.truncate(size as usize / std::mem::size_of::<T>().max(1));
    Some(v)
}

/// Read a property as raw bytes (8-byte aligned backing store).
pub fn get_bytes(id: AudioObjectID, a: AudioObjectPropertyAddress) -> Option<Vec<u8>> {
    let words: Vec<u64> = get_array(id, a)?;
    // SAFETY: reinterpreting initialised u64s as bytes.
    let bytes = unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * 8) };
    Some(bytes.to_vec())
}

/// Read a `CFStringRef` property (returned +1; released here).
pub fn get_string(id: AudioObjectID, a: AudioObjectPropertyAddress) -> Option<String> {
    let ptr: usize = get(id, a)?;
    // SAFETY: CoreAudio returns a retained CFString the caller must release.
    let owned = unsafe { cf::Owned::from_create(ptr as cf::CFTypeRef)? };
    // SAFETY: `owned` is a valid CF object.
    unsafe { cf::as_string(owned.as_ptr()) }
}

pub fn set<T: Copy>(id: AudioObjectID, a: AudioObjectPropertyAddress, value: T) -> OSStatus {
    // SAFETY: `value` is the property's exact C type.
    unsafe {
        AudioObjectSetPropertyData(
            id,
            &a,
            0,
            std::ptr::null(),
            std::mem::size_of::<T>() as u32,
            (&value as *const T).cast::<c_void>(),
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Devices
// ─────────────────────────────────────────────────────────────────────────────

/// Parse an `AudioBufferList` (64-bit layout) and sum its channel counts.
pub fn buffer_list_channels(bytes: &[u8]) -> u32 {
    let read = |off: usize| -> Option<u32> { Some(u32::from_ne_bytes(bytes.get(off..off + 4)?.try_into().ok()?)) };
    let Some(n) = read(0) else { return 0 };
    // `mBuffers` starts at offset 8 (pointer alignment); each AudioBuffer is
    // { UInt32 channels; UInt32 byteSize; void *data } = 16 bytes.
    (0..n.min(64) as usize).filter_map(|i| read(8 + i * 16)).sum()
}

pub fn default_output() -> Option<AudioObjectID> {
    get::<u32>(kAudioObjectSystemObject, global(kAudioHardwarePropertyDefaultOutputDevice)).filter(|id| *id != 0)
}

pub fn device_ids() -> Vec<AudioObjectID> {
    get_array::<u32>(kAudioObjectSystemObject, global(kAudioHardwarePropertyDevices)).unwrap_or_default()
}

pub fn output_channels(dev: AudioObjectID) -> u32 {
    get_bytes(dev, output(kAudioDevicePropertyStreamConfiguration)).map(|b| buffer_list_channels(&b)).unwrap_or(0)
}

/// `AudioValueRange`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct AudioValueRange {
    pub min: f64,
    pub max: f64,
}

/// `AudioStreamBasicDescription`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct AudioStreamBasicDescription {
    pub sample_rate: f64,
    pub format_id: u32,
    pub format_flags: u32,
    pub bytes_per_packet: u32,
    pub frames_per_packet: u32,
    pub bytes_per_frame: u32,
    pub channels_per_frame: u32,
    pub bits_per_channel: u32,
    pub reserved: u32,
}

pub fn bit_depth(dev: AudioObjectID) -> Option<u32> {
    let streams = get_array::<u32>(dev, output(kAudioDevicePropertyStreams))?;
    let first = *streams.first()?;
    get::<AudioStreamBasicDescription>(first, global(kAudioStreamPropertyPhysicalFormat))
        .map(|f| f.bits_per_channel)
        .filter(|b| *b > 0)
}

pub fn max_sample_rate(dev: AudioObjectID) -> Option<f64> {
    get_array::<AudioValueRange>(dev, global(kAudioDevicePropertyAvailableNominalSampleRates))?
        .iter()
        .map(|r| r.max)
        .filter(|r| r.is_finite() && *r > 0.0)
        .reduce(f64::max)
}

pub fn device_uid(dev: AudioObjectID) -> Option<String> {
    get_string(dev, global(kAudioDevicePropertyDeviceUID))
}

pub fn find_device_by_uid(uid: &str) -> Option<AudioObjectID> {
    device_ids().into_iter().find(|d| device_uid(*d).as_deref() == Some(uid))
}

/// Make `dev` the default output (and the alert sound device, when allowed).
pub fn set_default_output(dev: AudioObjectID) -> Result<(), OSStatus> {
    let status = set::<u32>(kAudioObjectSystemObject, global(kAudioHardwarePropertyDefaultOutputDevice), dev);
    if status != 0 {
        return Err(status);
    }
    if get::<u32>(dev, output(kAudioDevicePropertyDeviceCanBeDefaultSystemDevice)).unwrap_or(0) != 0 {
        let _ = set::<u32>(kAudioObjectSystemObject, global(kAudioHardwarePropertyDefaultSystemOutputDevice), dev);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Processes (macOS 14.2+)
// ─────────────────────────────────────────────────────────────────────────────

/// A CoreAudio process object.
#[derive(Debug, Clone)]
pub struct ProcessAudio {
    pub object: AudioObjectID,
    pub pid: i32,
    pub bundle_id: Option<String>,
    pub running_output: bool,
    pub output_devices: Vec<AudioObjectID>,
}

/// `None` when the system predates the process object API.
pub fn process_objects() -> Option<Vec<AudioObjectID>> {
    get_array::<u32>(kAudioObjectSystemObject, global(kAudioHardwarePropertyProcessObjectList))
}

pub fn process_info(object: AudioObjectID) -> Option<ProcessAudio> {
    let pid = get::<i32>(object, global(kAudioProcessPropertyPID))?;
    Some(ProcessAudio {
        object,
        pid,
        bundle_id: get_string(object, global(kAudioProcessPropertyBundleID)).filter(|s| !s.is_empty()),
        running_output: get::<u32>(object, global(kAudioProcessPropertyIsRunningOutput)).unwrap_or(0) != 0,
        output_devices: get_array::<u32>(object, output(kAudioProcessPropertyDevices)).unwrap_or_default(),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Listeners
// ─────────────────────────────────────────────────────────────────────────────

/// Registers `proc` for `address` on `object`. `client_data` must outlive the
/// registration (callers pass null and use a `static`).
pub fn add_listener(object: AudioObjectID, address: AudioObjectPropertyAddress, proc: ListenerProc) -> bool {
    // SAFETY: valid address; the proc is a 'static fn and takes no client data.
    unsafe { AudioObjectAddPropertyListener(object, &address, proc, std::ptr::null_mut()) == 0 }
}

pub fn remove_listener(object: AudioObjectID, address: AudioObjectPropertyAddress, proc: ListenerProc) {
    // SAFETY: mirrors `add_listener`.
    unsafe {
        AudioObjectRemovePropertyListener(object, &address, proc, std::ptr::null_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_list_layout() {
        let mut b = vec![0u8; 8 + 2 * 16];
        b[0..4].copy_from_slice(&2u32.to_ne_bytes());
        b[8..12].copy_from_slice(&2u32.to_ne_bytes());
        b[24..28].copy_from_slice(&6u32.to_ne_bytes());
        assert_eq!(buffer_list_channels(&b), 8);
        assert_eq!(buffer_list_channels(&[]), 0);
        assert_eq!(std::mem::size_of::<AudioStreamBasicDescription>(), 40);
    }
}
