//! Raw FFI used by providers. Everything here is macOS-only and kept minimal;
//! safe wrappers live next to their callers.

#[cfg(target_os = "macos")]
pub mod cf;
#[cfg(target_os = "macos")]
pub mod coreaudio;
#[cfg(target_os = "macos")]
pub mod iokit;
#[cfg(target_os = "macos")]
pub mod process;
