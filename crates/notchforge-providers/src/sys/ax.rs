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

/// A web page's media timeline (YouTube's "Seek slider" and the like), found
/// once in the browser's Accessibility tree and then read cheaply per poll.
pub struct WebSlider(Owned);

// SAFETY: AXUIElementRefs are immutable CF handles; the AX API may be called
// from any thread, one call at a time, which is how this is used.
unsafe impl Send for WebSlider {}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFNumberCreate(alloc: CFTypeRef, the_type: cf::CFIndex, value: *const std::ffi::c_void) -> CFTypeRef;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementSetAttributeValue(el: CFTypeRef, key: *const std::ffi::c_void, val: CFTypeRef) -> i32;
}

const K_CF_NUMBER_FLOAT64: cf::CFIndex = 6;

/// A numeric attribute (`AXValue`, `AXMaxValue`).
fn attribute_number(el: CFTypeRef, name: &str) -> Option<f64> {
    let key = cf::cfstring(name)?;
    let mut v: CFTypeRef = std::ptr::null();
    // SAFETY: valid element/key; the result is owned and type-checked.
    unsafe {
        if AXUIElementCopyAttributeValue(el, key.as_ptr(), &mut v) != 0 {
            return None;
        }
        let owned = Owned::from_create(v)?;
        if cf::CFGetTypeID(owned.as_ptr()) != cf::CFNumberGetTypeID() {
            return None;
        }
        let mut out = 0.0f64;
        let ok = cf::CFNumberGetValue(owned.as_ptr(), K_CF_NUMBER_FLOAT64, (&mut out as *mut f64).cast());
        (ok != 0 && out.is_finite()).then_some(out)
    }
}

/// Whether a slider's name says it is a media timeline.
pub fn is_timeline_name(name: &str) -> bool {
    let n = name.to_lowercase();
    !n.contains("volume")
        && ["seek", "progress", "playback", "position", "scrub", "timeline", "time slider", "track"]
            .iter()
            .any(|w| n.contains(w))
}

impl WebSlider {
    /// Find the playing page's timeline in `pid`'s windows. A bounded walk
    /// (the page tree can be large), so callers cache the result.
    pub fn find(pid: i32) -> Option<Self> {
        // SAFETY: AX calls on objects we own or borrow from owned arrays.
        unsafe {
            if AXIsProcessTrusted() == 0 {
                return None;
            }
            let app = Owned::from_create(AXUIElementCreateApplication(pid))?;
            // Chromium builds the page's tree only for assistive clients.
            if let (Some(k), Some(v)) = (cf::cfstring("AXManualAccessibility"), cf::cfstring("true")) {
                let _ = AXUIElementSetAttributeValue(app.as_ptr(), k.as_ptr(), v.as_ptr());
            }
            let key = cf::cfstring("AXWindows")?;
            let mut windows: CFTypeRef = std::ptr::null();
            if AXUIElementCopyAttributeValue(app.as_ptr(), key.as_ptr(), &mut windows) != 0 {
                return None;
            }
            let windows = Owned::from_create(windows)?;
            let mut budget = 9000usize;
            for i in 0..CFArrayGetCount(windows.as_ptr()).min(6) {
                let win = CFArrayGetValueAtIndex(windows.as_ptr(), i);
                if win.is_null() {
                    continue;
                }
                if let Some(found) = find_timeline(win, 0, &mut budget) {
                    return Some(Self(found));
                }
            }
            None
        }
    }

    /// `(position, duration)` in seconds; `None` once the element is gone.
    /// Native range inputs give numbers; ARIA sliders (YouTube) give only
    /// their value text, "1 Minutes 9 Seconds of 3 Minutes 35 Seconds".
    pub fn read(&self) -> Option<(f64, f64)> {
        let el = self.0.as_ptr();
        if let Some(r) = read_numbers(el) {
            return Some(r);
        }
        ["AXValue", "AXValueDescription"].iter().find_map(|a| attribute_string(el, a).and_then(|t| parse_time_text(&t)))
    }

    /// Step the slider toward `secs` with Accessibility increment/decrement
    /// actions, which ARIA sliders turn into arrow keys (5 s on YouTube).
    /// No page reload; the result lands within one step.
    pub fn step_to(&self, secs: f64) -> bool {
        unsafe extern "C" {
            fn AXUIElementPerformAction(el: CFTypeRef, action: CFTypeRef) -> i32;
        }
        let el = self.0.as_ptr();
        let (Some(inc), Some(dec)) = (cf::cfstring("AXIncrement"), cf::cfstring("AXDecrement")) else { return false };
        // SAFETY: valid element and action names.
        let act = |up: bool| unsafe {
            AXUIElementPerformAction(el, if up { inc.as_ptr() } else { dec.as_ptr() }.cast()) == 0
        };
        // The value text goes stale while the page hides its controls; one
        // step refreshes it, so the first read after it is the truth.
        let fresh_after = |prev: Option<f64>| {
            for _ in 0..12 {
                std::thread::sleep(std::time::Duration::from_millis(40));
                let now = self.read().map(|r| r.0);
                if now.is_some() && now != prev {
                    return now;
                }
            }
            self.read().map(|r| r.0)
        };
        let Some((start, dur)) = self.read() else { return false };
        let target = secs.clamp(0.0, dur);
        let up = target >= start;
        if !act(up) {
            return false;
        }
        let Some(p1) = fresh_after(Some(start)) else { return false };
        if !act(p1 <= target) {
            return true;
        }
        let Some(p2) = fresh_after(Some(p1)) else { return true };
        let step = (p2 - p1).abs().max(1.0);
        let remaining = target - p2;
        let n = (remaining.abs() / step).round() as usize;
        for _ in 0..n.min(2000) {
            if !act(remaining > 0.0) {
                break;
            }
        }
        true
    }
}

/// Numeric `(position, duration)` of a native range input.
fn read_numbers(el: CFTypeRef) -> Option<(f64, f64)> {
    {
        let max = attribute_number(el, "AXMaxValue")?;
        let min = attribute_number(el, "AXMinValue").unwrap_or(0.0);
        let value = attribute_number(el, "AXValue")?;
        let span = max - min;
        if span <= 0.5 {
            return None;
        }
        // Some players count milliseconds; nothing plays for 14 hours.
        let scale = if span > 50_000.0 { 1000.0 } else { 1.0 };
        Some((((value - min) / scale).clamp(0.0, span / scale), span / scale))
    }
}

/// Seconds in a slider's value text: "1 Minutes 9 Seconds of 3 Minutes 35
/// Seconds" (any language whose units start h / m / s), or "1:09 / 3:35".
/// Returns `(position, duration)`.
pub fn parse_time_text(text: &str) -> Option<(f64, f64)> {
    let t = text.to_lowercase();
    // Clock form first: the first two h:mm:ss groups.
    let clocks: Vec<f64> = t
        .split(|c: char| !(c.is_ascii_digit() || c == ':'))
        .filter(|w| w.contains(':'))
        .filter_map(|w| {
            let parts: Vec<f64> = w.split(':').map(|p| p.parse::<f64>().ok()).collect::<Option<_>>()?;
            (2..=3).contains(&parts.len()).then(|| parts.iter().fold(0.0, |acc, p| acc * 60.0 + p))
        })
        .collect();
    if clocks.len() >= 2 && clocks[1] > 0.0 {
        return Some((clocks[0].min(clocks[1]), clocks[1]));
    }
    // Words: number + unit pairs; a unit no smaller than the last one starts
    // the second time ("…9 seconds of 3 minutes…").
    let words: Vec<&str> = t.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()).collect();
    let mut groups: Vec<f64> = vec![0.0];
    let mut last_rank = 9;
    let mut seen = false;
    for pair in words.windows(2) {
        let Ok(n) = pair[0].parse::<f64>() else { continue };
        let (rank, mult) = match pair[1].chars().next() {
            Some('h') | Some('s') if pair[1].starts_with("st") => (2, 3600.0), // Stunden
            Some('h') => (2, 3600.0),
            Some('m') => (1, 60.0),
            Some('s') => (0, 1.0),
            _ => continue,
        };
        if seen && rank >= last_rank {
            groups.push(0.0);
        }
        *groups.last_mut()? += n * mult;
        last_rank = rank;
        seen = true;
    }
    match groups.as_slice() {
        [pos, dur, ..] if *dur > 0.0 => Some((pos.min(*dur), *dur)),
        _ => None,
    }
}

impl WebSlider {
    /// Move the timeline (works for native range inputs; ARIA sliders
    /// usually ignore it). True when the browser reports the value moved.
    pub fn set(&self, secs: f64) -> bool {
        let el = self.0.as_ptr();
        if read_numbers(el).is_none() {
            return false; // an ARIA slider: see `step_to`
        }
        let Some(max) = attribute_number(el, "AXMaxValue") else { return false };
        let min = attribute_number(el, "AXMinValue").unwrap_or(0.0);
        let scale = if max - min > 50_000.0 { 1000.0 } else { 1.0 };
        let target = (min + secs * scale).clamp(min, max);
        let Some(key) = cf::cfstring("AXValue") else { return false };
        // SAFETY: a +1 CFNumber we own, passed to a valid element.
        unsafe {
            let Some(n) = Owned::from_create(CFNumberCreate(
                std::ptr::null(),
                K_CF_NUMBER_FLOAT64,
                (&target as *const f64).cast(),
            )) else {
                return false;
            };
            if AXUIElementSetAttributeValue(el, key.as_ptr(), n.as_ptr()) != 0 {
                return false;
            }
        }
        attribute_number(el, "AXValue").is_some_and(|v| (v - target).abs() <= 2.0 * scale)
    }
}

/// Depth-first search for a slider named like a media timeline.
unsafe fn find_timeline(el: CFTypeRef, depth: usize, budget: &mut usize) -> Option<Owned> {
    if depth > 40 || *budget == 0 {
        return None;
    }
    *budget -= 1;
    let role = attribute_string(el, "AXRole").unwrap_or_default();
    if role == "AXSlider" {
        let name = ["AXDescription", "AXTitle", "AXHelp"]
            .iter()
            .filter_map(|a| attribute_string(el, a))
            .collect::<Vec<_>>()
            .join(" ");
        let readable = read_numbers(el).is_some()
            || ["AXValue", "AXValueDescription"]
                .iter()
                .any(|a| attribute_string(el, a).and_then(|t| parse_time_text(&t)).is_some());
        if is_timeline_name(&name) && readable {
            // SAFETY: borrowed element; retain it for the cache.
            unsafe extern "C" {
                fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
            }
            return unsafe { Owned::from_create(CFRetain(el)) };
        }
        return None;
    }
    // Nothing inside these is a page's media timeline.
    if matches!(role.as_str(), "AXMenuBar" | "AXToolbar" | "AXTabGroup" | "AXStaticText" | "AXImage" | "AXLink") {
        return None;
    }
    let key = cf::cfstring("AXChildren")?;
    let mut kids: CFTypeRef = std::ptr::null();
    // SAFETY: valid element/key; result owned.
    if unsafe { AXUIElementCopyAttributeValue(el, key.as_ptr(), &mut kids) } != 0 {
        return None;
    }
    let kids = unsafe { Owned::from_create(kids) }?;
    for i in 0..unsafe { CFArrayGetCount(kids.as_ptr()) }.min(400) {
        let child = unsafe { CFArrayGetValueAtIndex(kids.as_ptr(), i) };
        if !child.is_null() {
            if let Some(found) = unsafe { find_timeline(child, depth + 1, budget) } {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(test)]
mod timeline_tests {
    use super::is_timeline_name;

    #[test]
    fn reads_time_texts() {
        use super::parse_time_text as p;
        assert_eq!(p("0 Minutes 9 Seconds of 3 Minutes 35 Seconds"), Some((9.0, 215.0)));
        assert_eq!(p("9 Seconds of 3 Minutes 35 Seconds"), Some((9.0, 215.0)));
        assert_eq!(p("1 Hours 2 Minutes 3 Seconds of 2 Hours 0 Minutes 0 Seconds"), Some((3723.0, 7200.0)));
        assert_eq!(p("3 minutes 5 secondes sur 4 minutes"), Some((185.0, 240.0)));
        assert_eq!(p("1:09 / 3:35"), Some((69.0, 215.0)));
        assert_eq!(p("1:02:03 of 2:00:00"), Some((3723.0, 7200.0)));
        assert_eq!(p("Seek slider"), None);
        assert_eq!(p("42%"), None);
    }

    #[test]
    fn names_timelines_not_volume() {
        assert!(is_timeline_name("Seek slider"));
        assert!(is_timeline_name("Playback progress"));
        assert!(!is_timeline_name("Volume"));
        assert!(!is_timeline_name("Volume slider"));
        assert!(!is_timeline_name("Brightness"));
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
