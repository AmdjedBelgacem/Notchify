//! The pieces every space of the open island is built from, after the
//! *Notchisland redesign* boards: true black ground, one flat tile colour, one
//! cyan accent for the thing to do, amber for "needs you", green for "done".
//!
//! Spaces lay out in a single column of sections. A section is a small
//! heading and then rows or tiles; rows are 44–56 pt tall, separated by a
//! hairline, and every action is a pill button.

use crate::ui::{FlippedView, Targets, Weight, attributed, font, label, mono_digits, rect, rgba, shape_view};
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSButton, NSColor, NSScrollView, NSScrollerStyle, NSTextField, NSView};
use objc2_foundation::NSRect;

/// Space around a space's content, and between its sections.
pub const PAD: f64 = 16.0;
/// Between rows and tiles inside a section.
pub const GAP: f64 = 10.0;
/// Corner radius of tiles.
pub const TILE_R: f64 = 16.0;
/// A pill button's height.
pub const BUTTON_H: f64 = 30.0;

thread_local! {
    /// The accent the user picked (Settings › Look), as RGB.
    static ACCENT: std::cell::Cell<(f64, f64, f64)> = const { std::cell::Cell::new(CYAN) };
    /// Motion is reduced (the setting, or the system's when it follows it).
    static REDUCE_MOTION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Set whether looping motion (progress sweeps) should hold still.
pub fn set_reduce_motion(on: bool) {
    REDUCE_MOTION.with(|r| r.set(on));
}

/// An indeterminate progress sweep, `w` wide: a short accent bar gliding
/// along a hairline track, looping on Core Animation (no redraws from us).
/// Its phase follows the media clock, so a sweep rebuilt with its list keeps
/// gliding from where the previous one was instead of jumping back.
pub fn sweep(mtm: MainThreadMarker, frame: NSRect) -> Retained<FlippedView> {
    use objc2_quartz_core::{CABasicAnimation, CAMediaTiming, CAMediaTimingFunction, kCAMediaTimingFunctionEaseInEaseOut};
    const PERIOD: f64 = 1.6;
    let w = frame.size.width;
    let h = frame.size.height;
    let track = shape_view(mtm, frame, &hairline(), h / 2.0);
    if let Some(l) = track.layer() {
        l.setMasksToBounds(true);
    }
    let bar_w = (w * 0.3).max(24.0);
    let still = REDUCE_MOTION.with(|r| r.get());
    let bar = shape_view(mtm, rect(if still { (w - bar_w) / 2.0 } else { -bar_w }, 0.0, bar_w, h), &accent(), h / 2.0);
    track.addSubview(&bar);
    if !still {
        if let Some(l) = bar.layer() {
            let anim = CABasicAnimation::animationWithKeyPath(Some(&objc2_foundation::NSString::from_str(
                "transform.translation.x",
            )));
            let from = objc2_foundation::NSNumber::new_f64(0.0);
            let to = objc2_foundation::NSNumber::new_f64(w + bar_w);
            // SAFETY: NSNumber values for a scalar key path.
            unsafe {
                anim.setFromValue(Some(&from));
                anim.setToValue(Some(&to));
            }
            anim.setDuration(PERIOD);
            anim.setRepeatCount(f32::INFINITY);
            // SAFETY: a system timing function name.
            let ease = unsafe { CAMediaTimingFunction::functionWithName(kCAMediaTimingFunctionEaseInEaseOut) };
            anim.setTimingFunction(Some(&ease));
            anim.setTimeOffset(objc2_quartz_core::CACurrentMediaTime() % PERIOD);
            anim.setRemovedOnCompletion(false);
            l.addAnimation_forKey(&anim, Some(&objc2_foundation::NSString::from_str("sweep")));
        }
    }
    track
}

/// The island's own accent, and the "system" choice.
const CYAN: (f64, f64, f64) = (0.392, 0.824, 1.0);

/// The accent choices Settings offers: (config value, name, RGB).
pub const ACCENTS: [(&str, &str, (f64, f64, f64)); 5] = [
    ("system", "Cyan", CYAN),
    ("sunset", "Sunset", (1.0, 0.584, 0.275)),
    ("ocean", "Ocean", (0.369, 0.588, 1.0)),
    ("violet", "Violet", (0.749, 0.553, 1.0)),
    ("forest", "Forest", (0.365, 0.851, 0.533)),
];

/// RGB for a config accent: a preset name or a `#rrggbb` hex.
pub fn accent_rgb(value: &str) -> (f64, f64, f64) {
    if let Some(&(_, _, rgb)) = ACCENTS.iter().find(|(k, _, _)| *k == value) {
        return rgb;
    }
    let h = value.trim_start_matches('#');
    let part = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok()).map(|v| f64::from(v) / 255.0);
    match (h.len(), part(0), part(2), part(4)) {
        (6, Some(r), Some(g), Some(b)) => (r, g, b),
        _ => CYAN,
    }
}

/// Use the accent from the config for every view built from now on.
pub fn set_accent(value: &str) {
    ACCENT.with(|a| a.set(accent_rgb(value)));
}

pub fn accent() -> Retained<NSColor> {
    let (r, g, b) = ACCENT.with(std::cell::Cell::get);
    rgba(r, g, b, 1.0)
}
/// Text on the accent: near black, for contrast.
pub fn on_accent() -> Retained<NSColor> {
    rgba(0.0, 0.075, 0.11, 1.0)
}
pub fn amber() -> Retained<NSColor> {
    rgba(1.0, 0.839, 0.039, 1.0)
}
pub fn green() -> Retained<NSColor> {
    rgba(0.188, 0.82, 0.345, 1.0)
}
pub fn red() -> Retained<NSColor> {
    rgba(1.0, 0.271, 0.227, 1.0)
}
/// The one surface on black.
pub fn tile() -> Retained<NSColor> {
    rgba(0.086, 0.086, 0.094, 1.0)
}
/// Buttons and the selected tab: a step lighter than a tile.
pub fn raised() -> Retained<NSColor> {
    rgba(0.173, 0.173, 0.188, 1.0)
}
/// Hairlines between rows.
pub fn hairline() -> Retained<NSColor> {
    rgba(0.11, 0.11, 0.122, 1.0)
}
pub fn text() -> Retained<NSColor> {
    rgba(0.96, 0.96, 0.97, 1.0)
}
/// Second line of a row: 7:1 or better on black and on a tile.
pub fn text2() -> Retained<NSColor> {
    rgba(0.631, 0.631, 0.651, 1.0)
}
/// Headings, times and hints.
pub fn text3() -> Retained<NSColor> {
    rgba(0.557, 0.557, 0.576, 1.0)
}

/// A flat tile.
pub fn tile_view(mtm: MainThreadMarker, frame: NSRect) -> Retained<FlippedView> {
    shape_view(mtm, frame, &tile(), TILE_R)
}

/// A section's small heading, e.g. "Today".
pub fn heading(mtm: MainThreadMarker, title: &str, frame: NSRect) -> Retained<NSTextField> {
    label(mtm, title, &font(12.0, Weight::Semibold), &text3(), frame)
}

/// A filled circle `d` pt across at (`x`, `y`).
pub fn dot(mtm: MainThreadMarker, x: f64, y: f64, d: f64, color: &NSColor) -> Retained<FlippedView> {
    shape_view(mtm, rect(x, y, d, d), color, d / 2.0)
}

/// How a pill button reads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    /// The one thing to do: cyan with dark text.
    Primary,
    /// Everything else: a lighter grey pill.
    Secondary,
}

/// A pill button running `f` on click.
pub fn button(
    mtm: MainThreadMarker,
    targets: &Targets,
    title: &str,
    kind: Btn,
    frame: NSRect,
    f: impl Fn() + 'static,
) -> Retained<NSButton> {
    let t = targets.add(mtm, move |_| f());
    let (fg, bg) = match kind {
        Btn::Primary => (on_accent(), accent()),
        Btn::Secondary => (text(), raised()),
    };
    let fs = if frame.size.height < 28.0 { 12.0 } else { 13.0 };
    crate::ui::pill_button(mtm, title, None, &font(fs, Weight::Semibold), &fg, &bg, frame.size.height / 2.0, &t, frame)
}

/// The width a pill button needs for `title`.
pub fn button_width(title: &str, h: f64) -> f64 {
    let fs = if h < 28.0 { 12.0 } else { 13.0 };
    crate::ui::text_width(&attributed(title, &font(fs, Weight::Semibold), &text())).ceil() + 28.0
}

/// A round icon button (play, pause, skip…).
pub fn round_button(
    mtm: MainThreadMarker,
    targets: &Targets,
    symbol: &str,
    tip: &str,
    filled: bool,
    frame: NSRect,
    f: impl Fn() + 'static,
) -> Retained<NSButton> {
    let t = targets.add(mtm, move |_| f());
    let d = frame.size.width;
    let tint = if filled { NSColor::blackColor() } else { text() };
    let b = crate::ui::symbol_button(mtm, symbol, (d * 0.36).max(10.0), tip, &tint, &t, frame);
    let bg = if filled { text() } else { rgba(0.0, 0.0, 0.0, 0.0) };
    crate::ui::make_card(&b, &bg, d / 2.0);
    b
}

/// A progress bar: a track and its filled part (`fraction` 0–1).
pub fn bar(mtm: MainThreadMarker, frame: NSRect, fraction: f64, fill: &NSColor) -> Retained<FlippedView> {
    let h = frame.size.height;
    let track = shape_view(mtm, frame, &raised(), h / 2.0);
    let w = (frame.size.width * fraction.clamp(0.0, 1.0)).max(if fraction > 0.0 { h } else { 0.0 });
    track.addSubview(&shape_view(mtm, rect(0.0, 0.0, w, h), fill, h / 2.0));
    track
}

/// A command or path, in mono on black inside a tile.
pub fn mono_box(mtm: MainThreadMarker, line: &str, frame: NSRect) -> Retained<FlippedView> {
    let b = shape_view(mtm, frame, &NSColor::blackColor(), 10.0);
    let l = label(
        mtm,
        line,
        &mono_digits(12.5, Weight::Regular),
        &rgba(0.898, 0.898, 0.918, 1.0),
        rect(12.0, (frame.size.height - 17.0) / 2.0, frame.size.width - 24.0, 17.0),
    );
    l.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
    b.addSubview(&l);
    b
}

/// A one-line label that truncates at its end.
pub fn line(
    mtm: MainThreadMarker,
    s: &str,
    size: f64,
    w: Weight,
    color: &NSColor,
    frame: NSRect,
) -> Retained<NSTextField> {
    let l = label(mtm, s, &font(size, w), color, frame);
    l.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
    l
}

/// A hairline across `w` at `y`.
pub fn rule(mtm: MainThreadMarker, x: f64, y: f64, w: f64) -> Retained<FlippedView> {
    shape_view(mtm, rect(x, y, w, 1.0), &hairline(), 0.0)
}

/// Room to the right of a scrolling column for the overlay scroller, so it never sits on content.
pub const GUTTER: f64 = 10.0;

/// A vertically scrolling container for a space's column. `frame` is the content's area; the
/// scroll view reaches `GUTTER` past it so the scroller runs in the margin, not over the rows.
pub fn scroller(mtm: MainThreadMarker, frame: NSRect, doc: &NSView) -> Retained<NSScrollView> {
    let frame = NSRect::new(frame.origin, objc2_foundation::NSSize::new(frame.size.width + GUTTER, frame.size.height));
    let s = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    s.setDrawsBackground(false);
    s.setHasVerticalScroller(true);
    s.setAutohidesScrollers(true);
    s.setScrollerStyle(NSScrollerStyle::Overlay);
    s.setDocumentView(Some(doc));
    s
}

const DAY_MS: i64 = 86_400_000;

/// "14:32", local time.
pub fn clock(ms: i64) -> String {
    let (day, _) = notchforge_providers::calendar::local_day(ms);
    let mins = (ms - day).rem_euclid(DAY_MS) / 60_000;
    format!("{:02}:{:02}", mins / 60, mins % 60)
}

/// When something is due, in a few words: "Today, 12:00", "Tomorrow", "In 3 days".
pub fn due_label(due_ms: i64, now_ms: i64) -> String {
    let (today, _) = notchforge_providers::calendar::local_day(now_ms);
    let (due_day, _) = notchforge_providers::calendar::local_day(due_ms);
    let days = (due_day - today).div_euclid(DAY_MS);
    let time = if due_ms != due_day { format!(", {}", clock(due_ms)) } else { String::new() };
    match days {
        d if d < 0 => format!("Overdue{time}"),
        0 => format!("Today{time}"),
        1 => format!("Tomorrow{time}"),
        d => format!("In {d} days"),
    }
}

/// "now", "4 min", "2 h" or the clock time for older.
pub fn ago(now_ms: i64, at_ms: i64) -> String {
    let mins = (now_ms - at_ms).max(0) / 60_000;
    match mins {
        0 => "now".into(),
        1..=59 => format!("{mins} min"),
        60..=719 => format!("{} h", mins / 60),
        _ => clock(at_ms),
    }
}

/// "now", "in 28 min", "in 2 h".
pub fn until(now_ms: i64, at_ms: i64) -> String {
    let mins = (at_ms - now_ms) / 60_000;
    match mins {
        i64::MIN..=0 => "now".into(),
        1..=59 => format!("in {mins} min"),
        _ => format!("in {} h", (mins + 30) / 60),
    }
}

/// "2 min", "1 h 05".
pub fn span(ms: i64) -> String {
    let mins = ms.max(0) / 60_000;
    if mins < 60 { format!("{} min", mins.max(1)) } else { format!("{} h {:02}", mins / 60, mins % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents_read_presets_and_hex() {
        assert_eq!(accent_rgb("system"), CYAN);
        assert_eq!(accent_rgb("forest"), ACCENTS[4].2);
        assert_eq!(accent_rgb("#ff0000"), (1.0, 0.0, 0.0));
        assert_eq!(accent_rgb("#zz"), CYAN);
        assert_eq!(accent_rgb("nonsense"), CYAN);
    }

    #[test]
    fn times_read_plainly() {
        let m = 60_000;
        assert_eq!(ago(10 * m, 10 * m), "now");
        assert_eq!(ago(10 * m, 6 * m), "4 min");
        assert_eq!(ago(200 * m, 80 * m), "2 h");
        assert_eq!(until(0, 28 * m), "in 28 min");
        assert_eq!(until(0, 150 * m), "in 3 h");
        assert_eq!(until(5 * m, 0), "now");
        assert_eq!(span(30_000), "1 min");
        assert_eq!(span(65 * m), "1 h 05");
    }

    #[test]
    fn due_dates_read_plainly() {
        let now = notchforge_core::time::now_ms();
        let (today, _) = notchforge_providers::calendar::local_day(now);
        assert_eq!(due_label(today, now), "Today");
        assert_eq!(due_label(today + 12 * 3_600_000, now), "Today, 12:00");
        assert_eq!(due_label(today + DAY_MS, now), "Tomorrow");
        assert_eq!(due_label(today - DAY_MS, now), "Overdue");
        assert_eq!(due_label(today + 4 * DAY_MS, now), "In 4 days");
    }
}
