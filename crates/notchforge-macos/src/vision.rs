//! Text recognition via the Vision framework.
//!
//! Vision is loaded at runtime (`dlopen`) rather than linked, so the app
//! never pays for it at startup and still runs if Apple reshuffles private
//! symbols. The classes used (`VNRecognizeTextRequest`,
//! `VNImageRequestHandler`) are public framework classes; the messaging is
//! plain `objc2` with `objc_getClass`. Best effort: `None` on any failure.

use std::ffi::c_void;
use std::sync::OnceLock;

use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::msg_send;

static VISION: OnceLock<bool> = OnceLock::new();

fn cls(name: &str) -> Option<&'static AnyClass> {
    let c = std::ffi::CString::new(name).ok()?;
    AnyClass::get(c.as_c_str())
}

/// Load the framework once; true when its classes registered.
fn vision_available() -> bool {
    *VISION.get_or_init(|| {
        // SAFETY: a system framework with a stable path; loading twice is a
        // no-op.
        const VISION_FRAMEWORK: &std::ffi::CStr =
            c"/System/Library/Frameworks/Vision.framework/Vision";
        let handle = unsafe { libc::dlopen(VISION_FRAMEWORK.as_ptr().cast(), libc::RTLD_LAZY) };
        !handle.is_null() && cls("VNRecognizeTextRequest").is_some()
    })
}

/// Recognize text in the image at `path`, joined with newlines.
pub fn recognize_text(path: &std::path::Path) -> Option<String> {
    if !vision_available() {
        return None;
    }
    let img = crate::icons::file_image(path)?;
    let cg = unsafe { img.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }?;
    // SAFETY: a live CGImage reference owned by `img` for this call.
    unsafe { recognize_cgimage(&*cg as *const objc2_core_graphics::CGImage as *const c_void) }
}

/// SAFETY: `cg` must be a live CGImage (always is, coming from NSImage).
unsafe fn recognize_cgimage(cg: *const c_void) -> Option<String> {
    let request_cls = cls("VNRecognizeTextRequest")?;
    let request: *mut AnyObject = msg_send![request_cls, alloc];
    let request: *mut AnyObject = msg_send![request, init];
    // VNRequestTextRecognitionLevelAccurate = 1.
    let _: () = msg_send![request, setRecognitionLevel: 1i64];
    let _: () = msg_send![request, setUsesLanguageCorrection: Bool::YES];

    let handler_cls = cls("VNImageRequestHandler")?;
    let handler: *mut AnyObject = msg_send![handler_cls, alloc];
    let handler: *mut AnyObject =
        msg_send![handler, initWithCGImage: cg as *mut AnyObject, options: std::ptr::null_mut::<AnyObject>()];

    let requests: *mut AnyObject = msg_send![cls("NSMutableArray").expect("NSMutableArray"), array];
    let _: () = msg_send![requests, addObject: request];

    let mut error: *mut AnyObject = std::ptr::null_mut();
    let ok: Bool = msg_send![handler, performRequests: requests, error: &mut error];
    if !ok.as_bool() {
        return None;
    }

    let results: *mut AnyObject = msg_send![request, results];
    let count: usize = msg_send![results, count];
    let mut out = String::new();
    for i in 0..count {
        let obs: *mut AnyObject = msg_send![results, objectAtIndex: i];
        let candidates: *mut AnyObject = msg_send![obs, topCandidates: 1usize];
        let n: usize = msg_send![candidates, count];
        if n == 0 {
            continue;
        }
        let first: *mut AnyObject = msg_send![candidates, firstObject];
        let text: *mut objc2_foundation::NSString = msg_send![first, string];
        if !out.is_empty() {
            out.push('\n');
        }
        // SAFETY: Vision returned a live NSString for a non-empty candidate.
        if let Some(t) = unsafe { text.as_ref() } {
            out.push_str(t.to_string().as_str());
        }
    }
    (!out.trim().is_empty()).then_some(out)
}
