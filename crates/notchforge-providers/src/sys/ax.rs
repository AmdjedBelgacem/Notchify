//! Minimal Accessibility reads used by providers.
//!
//! Only one thing is needed today: the window titles of a running app, as the
//! fallback for browser tab titles when the browser has no usable scripting
//! dictionary (Firefox) or Automation was not granted. Every browser draws
//! its tab title into its window title, and Chromium browsers expose the AX
//! tree once woken with `AXManualAccessibility`.

use super::cf::{self, CFArrayGetCount, CFArrayGetValueAtIndex, CFTypeRef, Owned};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub fn AXIsProcessTrusted() -> u8;
    pub fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    pub fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: *const std::ffi::c_void,
        value: *mut CFTypeRef,
    ) -> i32;
}


/// A string attribute of an AX element, empty-to-None.
pub(crate) fn attribute_string(el: CFTypeRef, name: &str) -> Option<String> {
    let key = cf::cfstring(name)?;
    let mut v: CFTypeRef = std::ptr::null();
    // SAFETY: valid element/key; result owned.
    unsafe {
        if AXUIElementCopyAttributeValue(el, key.as_ptr(), &mut v) != 0 {
            return None;
        }
        let owned = Owned::from_create(v)?;
        let p = owned.as_ptr();
        unsafe extern "C" {
            fn CFGetTypeID(cf: CFTypeRef) -> usize;
            fn CFStringGetTypeID() -> usize;
        }
        if CFGetTypeID(p) != CFStringGetTypeID() {
            return None;
        }
        let len = cf::CFStringGetLength(p as cf::CFStringRef);
        let mut buf = vec![0u8; (len as usize + 1) * 4];
        if cf::CFStringGetCString(
            p as cf::CFStringRef,
            buf.as_mut_ptr() as *mut std::ffi::c_char,
            buf.len() as cf::CFIndex,
            cf::kCFStringEncodingUTF8,
        ) == 0
        {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        Some(String::from_utf8_lossy(&buf[..end]).into_owned())
    }
}

/// The web page titles of `pid`'s windows, frontmost first: Chromium puts the
/// page's own `<title>` on its `AXWebArea` — longer and more current than the
/// window title, which macOS truncates. Empty without Accessibility.
pub fn web_titles(pid: i32) -> Vec<String> {
    // SAFETY: AX calls on objects we own or borrow from owned arrays.
    unsafe {
        if AXIsProcessTrusted() == 0 {
            return Vec::new();
        }
        let app = AXUIElementCreateApplication(pid);
        let Some(app) = Owned::from_create(app) else { return Vec::new() };
        let Some(windows_key) = cf::cfstring("AXWindows") else { return Vec::new() };
        let mut windows: CFTypeRef = std::ptr::null();
        if AXUIElementCopyAttributeValue(app.as_ptr(), windows_key.as_ptr(), &mut windows) != 0 {
            return Vec::new();
        }
        let Some(windows) = Owned::from_create(windows) else { return Vec::new() };
        let mut out = Vec::new();
        for i in 0..CFArrayGetCount(windows.as_ptr()).min(8) {
            let win = CFArrayGetValueAtIndex(windows.as_ptr(), i);
            if win.is_null() {
                continue;
            }
            // window → scroll area → web area (Chromium's shape); a short walk
            // with a role filter keeps this cheap.
            collect_web_titles(win, 0, &mut out);
            if out.len() >= 12 {
                break;
            }
        }
        out
    }
}

/// Depth-limited descent collecting `AXWebArea` titles.
unsafe fn collect_web_titles(el: CFTypeRef, depth: usize, out: &mut Vec<String>) {
    if depth > 4 || out.len() >= 12 {
        return;
    }
    let role = attribute_string(el, "AXRole").unwrap_or_default();
    if role == "AXWebArea" {
        if let Some(t) = attribute_string(el, "AXTitle").filter(|t| !t.trim().is_empty()) {
            out.push(t.trim().to_owned());
            return; // no need to descend into the page itself
        }
    }
    let Some(key) = cf::cfstring("AXChildren") else { return };
    let mut kids: CFTypeRef = std::ptr::null();
    // SAFETY: valid element/key; result owned.
    if unsafe { AXUIElementCopyAttributeValue(el, key.as_ptr(), &mut kids) } != 0 {
        return;
    }
    let kids = unsafe { Owned::from_create(kids) };
    let Some(kids) = kids else { return };
    for i in 0..unsafe { CFArrayGetCount(kids.as_ptr()) }.min(24) {
        let child = unsafe { CFArrayGetValueAtIndex(kids.as_ptr(), i) };
        if !child.is_null() {
            unsafe { collect_web_titles(child, depth + 1, out) };
        }
    }
}

/// The window titles of `pid`'s app, frontmost window first. Empty when
/// Accessibility is unavailable or the app exposes nothing.
pub fn window_titles(pid: i32) -> Vec<String> {
    // SAFETY: AX calls on objects we own or borrow from owned arrays.
    unsafe {
        if AXIsProcessTrusted() == 0 {
            return Vec::new();
        }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Vec::new();
        }
        let app = Owned::from_create(app);
        let Some(app) = app else { return Vec::new() };
        let mut windows: CFTypeRef = std::ptr::null();
        let key = match cf::cfstring("AXWindows") {
            Some(k) => k,
            None => return Vec::new(),
        };
        if AXUIElementCopyAttributeValue(app.as_ptr(), key.as_ptr(), &mut windows) != 0 {
            return Vec::new();
        }
        let Some(windows) = Owned::from_create(windows) else { return Vec::new() };
        let mut out = Vec::new();
        for i in 0..CFArrayGetCount(windows.as_ptr()) {
            let win = CFArrayGetValueAtIndex(windows.as_ptr(), i);
            let Some(title_key) = cf::cfstring("AXTitle") else { break };
            let mut title: CFTypeRef = std::ptr::null();
            if AXUIElementCopyAttributeValue(win, title_key.as_ptr(), &mut title) != 0 {
                continue;
            }
            let Some(t) = Owned::from_create(title).and_then(|t| cf::as_string(t.as_ptr())) else { continue };
            if !t.trim().is_empty() {
                out.push(t.trim().to_owned());
            }
            if out.len() >= 12 {
                break;
            }
        }
        out
    }
}

#[cfg(test)]
mod probe {
    use super::*;
    unsafe extern "C" {
        fn AXUIElementSetAttributeValue(el: CFTypeRef, key: *const std::ffi::c_void, val: CFTypeRef) -> i32;
    }

    fn str_attr(el: CFTypeRef, name: &str) -> Option<String> {
        attribute_string(el, name)
    }

    fn walk(el: CFTypeRef, depth: usize, out: &mut Vec<String>, budget: &mut usize) {
        if depth > 14 || *budget == 0 {
            return;
        }
        *budget -= 1;
        let role = str_attr(el, "AXRole").unwrap_or_default();
        let title = str_attr(el, "AXTitle").unwrap_or_default();
        let desc = str_attr(el, "AXDescription").unwrap_or_default();
        let text = str_attr(el, "AXValue").unwrap_or_default();
        let hay = format!("{title} {desc} {text}");
        if hay.to_lowercase().contains("play")
            || hay.to_lowercase().contains("video")
            || hay.to_lowercase().contains("watch")
            || role.contains("Video")
        {
            out.push(format!("{}{role}: t={title:?} d={desc:?} v={text:?}", "  ".repeat(depth)));
        }
        // Raw skeleton so we can see how deep the web area goes.
        if std::env::var("AX_SKELETON").is_ok() && depth < 5 {
            out.push(format!("{}[{role}] t={title:?}", "  ".repeat(depth)));
        }
        let key = match cf::cfstring("AXChildren") {
            Some(k) => k,
            None => return,
        };
        let mut kids: CFTypeRef = std::ptr::null();
        // SAFETY: valid element/key; result owned.
        if unsafe { AXUIElementCopyAttributeValue(el, key.as_ptr(), &mut kids) } != 0 {
            return;
        }
        let Some(kids) = (unsafe { Owned::from_create(kids) }) else { return };
        let n = unsafe { CFArrayGetCount(kids.as_ptr()) };
        for i in 0..n.min(60) {
            let child = unsafe { CFArrayGetValueAtIndex(kids.as_ptr(), i) };
            if child.is_null() {
                continue;
            }
            walk(child, depth + 1, out, budget);
        }
    }

    #[test]
    #[ignore = "live: run while a browser shows a page"]
    fn reads_web_titles() {
        let pid = std::env::var("DIA_PID").ok().and_then(|p| p.parse::<i32>().ok()).unwrap_or(0);
        let titles = web_titles(pid);
        println!("=== {} web titles ===", titles.len());
        assert!(!titles.is_empty());
        for t in &titles {
            println!("{t}");
        }
    }

    #[test]
    #[ignore = "live diagnostics: run while Dia plays a YouTube feed video"]
    fn dumps_dia_player_elements() {
        let pid = std::env::var("DIA_PID").ok().and_then(|p| p.parse::<i32>().ok()).unwrap_or(73281);
        // SAFETY: AX calls on objects we own.
        unsafe {
            let app_el = AXUIElementCreateApplication(pid);
            // Wake Chromium's web-content AX tree (windows only, otherwise).
            let key = cf::cfstring("AXManualAccessibility").expect("key");
            let val = cf::cfstring("true").expect("val");
            AXUIElementSetAttributeValue(app_el, key.as_ptr(), val.as_ptr());
            let app = Owned::from_create(app_el).expect("app element");
            std::thread::sleep(std::time::Duration::from_millis(600));
            let mut out = Vec::new();
            walk(app.as_ptr(), 0, &mut out, &mut 12000);
            println!("=== {} player-ish elements ===", out.len());
            for l in out.iter().take(120) {
                println!("{l}");
            }
        }
    }
}