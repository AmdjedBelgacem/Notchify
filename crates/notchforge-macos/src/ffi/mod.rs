//! Raw FFI to C frameworks that `objc2` does not cover (or covers with APIs we
//! don't need). Each submodule is minimal; safe wrappers live in the modules
//! that use them.

pub mod ax;
pub mod cg;
pub mod coreaudio;

/// CoreFoundation helpers are shared with the providers crate.
pub use notchforge_providers::sys::cf;
