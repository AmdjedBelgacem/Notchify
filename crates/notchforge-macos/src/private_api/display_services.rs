//! Built-in display brightness via the private `DisplayServices` framework.
//!
//! Apple Silicon Macs have no public API to read/set the built-in panel's
//! brightness (`IODisplayGetFloatParameter` only works on Intel). Control
//! Center and most third-party utilities use `DisplayServicesGetBrightness` /
//! `DisplayServicesSetBrightness`; so do we, loaded lazily with `dlopen`.

use std::ffi::{CStr, c_void};
use std::sync::OnceLock;

type GetFn = unsafe extern "C" fn(display: u32, brightness: *mut f32) -> i32;
type SetFn = unsafe extern "C" fn(display: u32, brightness: f32) -> i32;

struct Api {
    get: GetFn,
    set: SetFn,
}

const FRAMEWORK: &CStr = c"/System/Library/PrivateFrameworks/DisplayServices.framework/DisplayServices";

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        // SAFETY: dlopen/dlsym with valid NUL-terminated strings; the handle is
        // intentionally never closed so the function pointers stay valid.
        unsafe {
            let handle = libc::dlopen(FRAMEWORK.as_ptr(), libc::RTLD_LAZY);
            if handle.is_null() {
                tracing::info!("DisplayServices unavailable; brightness control disabled");
                return None;
            }
            let get = libc::dlsym(handle, c"DisplayServicesGetBrightness".as_ptr());
            let set = libc::dlsym(handle, c"DisplayServicesSetBrightness".as_ptr());
            if get.is_null() || set.is_null() {
                tracing::info!("DisplayServices brightness symbols missing");
                return None;
            }
            Some(Api {
                get: std::mem::transmute::<*mut c_void, GetFn>(get),
                set: std::mem::transmute::<*mut c_void, SetFn>(set),
            })
        }
    })
    .as_ref()
}

pub fn is_available() -> bool {
    api().is_some()
}

/// Brightness 0.0–1.0, or `None` if unsupported for this display.
pub fn get(display: u32) -> Option<f32> {
    let api = api()?;
    let mut value = 0.0f32;
    // SAFETY: valid out-pointer; function signature matches the framework's.
    let err = unsafe { (api.get)(display, &mut value) };
    (err == 0).then_some(value.clamp(0.0, 1.0))
}

pub fn set(display: u32, value: f32) -> bool {
    let Some(api) = api() else { return false };
    // SAFETY: plain value arguments.
    unsafe { (api.set)(display, value.clamp(0.0, 1.0)) == 0 }
}
