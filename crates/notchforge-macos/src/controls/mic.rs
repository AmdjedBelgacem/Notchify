//! Mute state of the default microphone (CoreAudio, public API).

use crate::ffi::coreaudio::{self as ca, AudioObjectID};

fn default_input_device() -> Option<AudioObjectID> {
    ca::default_input()
}

/// `Some(muted)` when the default input exists and has a mute control;
/// `None` when there is nothing to mute (no mic, or a device without one).
pub fn muted() -> Option<bool> {
    let dev = default_input_device()?;
    if !ca::has(dev, ca::input(ca::kAudioDevicePropertyMute)) {
        return None;
    }
    Some(ca::get::<u32>(dev, ca::input(ca::kAudioDevicePropertyMute)).unwrap_or(0) != 0)
}

pub fn set_muted(muted: bool) -> bool {
    ca::set_microphone_muted(muted).is_ok()
}

/// Flip the current mute state; the new state, or `None` if unmutable.
pub fn toggle() -> Option<bool> {
    let next = !muted()?;
    set_muted(next).then_some(next)
}
