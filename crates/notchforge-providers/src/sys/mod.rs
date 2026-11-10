//! Raw FFI used by providers. Everything here is macOS-only and kept minimal;
//! safe wrappers live next to their callers.

#[cfg(target_os = "macos")]
pub mod ax;
#[cfg(target_os = "macos")]
pub mod cf;
#[cfg(target_os = "macos")]
pub mod cmio;
pub mod coreaudio;
#[cfg(target_os = "macos")]
pub mod iokit;
#[cfg(target_os = "macos")]
pub mod process;

/// Stand-ins so the providers build (and their pure logic is tested) on
/// other platforms; on those, nothing here finds anything.
#[cfg(not(target_os = "macos"))]
pub mod ax {
    pub struct WebSlider;
    impl WebSlider {
        pub fn find(_pid: i32) -> Option<Self> {
            None
        }
        pub fn read(&self) -> Option<(f64, f64)> {
            None
        }
        pub fn set(&self, _secs: f64) -> bool {
            false
        }
        pub fn step_to(&self, _secs: f64) -> bool {
            false
        }
    }
    pub fn web_titles(_pid: i32) -> Vec<String> {
        Vec::new()
    }
    pub fn window_titles(_pid: i32) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(not(target_os = "macos"))]
pub mod process {
    pub fn responsible_pid(pid: i32) -> i32 {
        pid
    }
    pub fn all_pids() -> Vec<i32> {
        Vec::new()
    }
    pub fn name(_pid: i32) -> Option<String> {
        None
    }
    pub fn args(_pid: i32) -> Option<String> {
        None
    }
    pub fn start_secs(_pid: i32) -> Option<i64> {
        None
    }
    pub fn is_app_running(_name: &str, _suffix: &str) -> bool {
        false
    }
}
