//! Collapsed state: the island hugs the notch; optional "wings" either side
//! show live signals (charging, low battery, tray contents, unread messages,
//! and the playing track's cover with an equaliser).
//!
//! The equaliser is four `CALayer` bars animated by Core Animation, which runs
//! in the render server: no timers or CPU in our process while music plays.

use super::{app_icon_view, set_app_icon};
use crate::ui::{
    FlippedView, Palette, Weight, label, mono_digits, rect, set_accessibility_label, set_symbol, set_text, symbol_view,
};
use notchforge_core::engine::Snapshot;
use notchforge_core::geometry::{LayoutKind, NotchLayout, WING_LEFT, WING_RIGHT};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSImageView, NSTextAlignment, NSTextField};
use objc2_foundation::NSPoint;

/// Per-render options from the controller.
#[derive(Debug, Clone, Copy)]
pub struct WingOptions {
    /// Show media (cover + equaliser) in the wings.
    pub media: bool,
    /// Animate the equaliser (off with Reduce Motion).
    pub animate: bool,
    /// `WING_LEFT` / `WING_RIGHT` mask of the wings actually shown; a wing
    /// whose bit is off (dodged beside menu bar items) hides its content.
    pub mask: u8,
    /// `WING_LEFT` / `WING_RIGHT` mask of sides whose content *wants* a wing
    /// but is dodged beside menu bar items: a chevron offers it on demand
    /// (the notch's version of the system's hidden-items chevron).
    pub chevrons: u8,
}

const EQ_HEIGHTS: [f64; 4] = [7.0, 12.0, 9.0, 11.0];
const EQ_DURATIONS: [f64; 4] = [0.42, 0.55, 0.38, 0.5];
const EQ_BAR: f64 = 2.5;
/// Below this shoulder width, text gives way to icons where both don't fit.
const NARROW: f64 = 52.0;
const EQ_GAP: f64 = 2.0;

pub struct CollapsedView {
    pub root: Retained<FlippedView>,
    art: Retained<NSImageView>,
    art_box: Retained<crate::ui::FlippedView>,
    eq: Retained<objc2_app_kit::NSView>,
    eq_bars: Vec<Retained<objc2_quartz_core::CALayer>>,
    eq_animating: std::cell::Cell<bool>,
    left_icon: Retained<NSImageView>,
    left_text: Retained<NSTextField>,
    right_icon: Retained<NSImageView>,
    right_text: Retained<NSTextField>,
    chev_l: Retained<NSImageView>,
    chev_r: Retained<NSImageView>,
    /// A count badge on the right icon's corner, for narrow shoulders where
    /// the count can't sit beside it.
    badge: Retained<FlippedView>,
    badge_text: Retained<NSTextField>,
    /// Width of each shoulder (the user's setting).
    wing: f64,
    /// Under a hardware notch: the chevrons' spot is behind the camera
    /// housing, so they are never drawn.
    physical: bool,
}

impl CollapsedView {
    pub fn new(mtm: MainThreadMarker, layout: &NotchLayout, p: &Palette) -> Self {
        // Always built at full wing width; the island clips wings whose mask
        // bit is off (see `Controller::apply_frame` for the offset).
        let s = layout.shape(LayoutKind::CollapsedWithWings(WING_LEFT | WING_RIGHT));
        let (w, h) = (s.width, s.height);
        let wing = layout.wing();
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        let icon = 15.0;
        let iy = (h - icon) / 2.0;
        let ty = (h - 15.0) / 2.0;
        // Narrow shoulders take a slightly smaller figure so a timer still fits.
        let f = mono_digits(if wing < NARROW { 10.5 } else { 11.5 }, Weight::Semibold);
        let left_icon = symbol_view(mtm, "bolt.fill", 12.0, &p.success, rect(12.0, iy, icon, icon), "Battery");
        let left_text = label(mtm, "", &f, &p.primary, rect(29.0, ty, wing - 31.0, 15.0));
        let right_icon = app_icon_view(mtm, rect(w - wing + 10.0, iy - 1.0, icon + 2.0, icon + 2.0));
        let right_text = label(mtm, "", &f, &p.primary, rect(w - wing + 30.0, ty, wing - 42.0, 15.0));
        right_text.setAlignment(NSTextAlignment::Right);
        let art_side = (h - 8.0).clamp(14.0, 20.0);
        // The image sits in a rounded container (its own layer clips it) rather
        // than carrying a layer itself, so it draws in every context.
        let art_box = crate::ui::shape_view(
            mtm,
            rect(12.0, (h - art_side) / 2.0, art_side, art_side),
            &crate::ui::rgba(0.0, 0.0, 0.0, 1.0),
            4.0,
        );
        if let Some(l) = art_box.layer() {
            l.setMasksToBounds(true);
        }
        let art = NSImageView::new(mtm);
        art.setImageScaling(objc2_app_kit::NSImageScaling::ScaleProportionallyUpOrDown);
        // A direct child of the root (nested inside the layer-backed box it
        // did not draw in offscreen renders); the box only backs it with black.
        art.setFrame(rect(12.0, (h - art_side) / 2.0, art_side, art_side));
        art.setHidden(true);

        // Equaliser: plain (y-up) view so bars grow from the bottom.
        let eq_w = 4.0 * EQ_BAR + 3.0 * EQ_GAP;
        let eq_h = 12.0;
        let eq = objc2_app_kit::NSView::initWithFrame(
            <objc2_app_kit::NSView as objc2::MainThreadOnly>::alloc(mtm),
            rect(w - 12.0 - eq_w, (h - eq_h) / 2.0, eq_w, eq_h),
        );
        eq.setWantsLayer(true);
        let mut eq_bars = Vec::new();
        if let Some(root_layer) = eq.layer() {
            for (i, bh) in EQ_HEIGHTS.iter().enumerate() {
                let bar = objc2_quartz_core::CALayer::new();
                bar.setBounds(rect(0.0, 0.0, EQ_BAR, *bh));
                bar.setAnchorPoint(objc2_foundation::NSPoint::new(0.5, 0.0));
                bar.setPosition(objc2_foundation::NSPoint::new(i as f64 * (EQ_BAR + EQ_GAP) + EQ_BAR / 2.0, 0.0));
                bar.setCornerRadius(EQ_BAR / 2.0);
                root_layer.addSublayer(&bar);
                eq_bars.push(bar);
            }
        }
        eq.setHidden(true);
        crate::ui::set_accessibility_label(&eq, "Music playing");

        // The notch's own "hidden items" chevron: shown on a side whose wing
        // is dodged beside menu bar items; clicking pops the wing out over
        // them for a moment. The double chevron points at the wing it brings
        // back, sitting just inside the notch's edge.
        let chev_size = 9.0;
        let chev_l = symbol_view(
            mtm,
            "chevron.left.2",
            chev_size,
            &p.secondary,
            rect(wing + 2.0, (h - chev_size) / 2.0, chev_size + 2.0, chev_size),
            "Show what the notch hid",
        );
        let chev_r = symbol_view(
            mtm,
            "chevron.right.2",
            chev_size,
            &p.secondary,
            rect(w - wing - chev_size - 4.0, (h - chev_size) / 2.0, chev_size + 2.0, chev_size),
            "Show what the notch hid",
        );
        chev_l.setHidden(true);
        chev_r.setHidden(true);

        let badge = crate::ui::shape_view(mtm, rect(0.0, 0.0, 13.0, 13.0), &crate::ui::rgba(1.0, 0.27, 0.23, 1.0), 6.5);
        let badge_text = label(
            mtm,
            "",
            &crate::ui::font(8.5, Weight::Bold),
            &crate::ui::rgba(1.0, 1.0, 1.0, 1.0),
            rect(0.0, 1.0, 13.0, 11.0),
        );
        badge_text.setAlignment(NSTextAlignment::Center);
        badge.addSubview(&badge_text);
        badge.setHidden(true);

        for v in [
            &*left_icon as &objc2_app_kit::NSView,
            &left_text,
            &right_icon,
            &right_text,
            &art_box,
            &art,
            &eq,
            &chev_l,
            &chev_r,
            &badge,
        ] {
            root.addSubview(v);
        }
        Self {
            root,
            art,
            art_box,
            eq,
            eq_bars,
            eq_animating: std::cell::Cell::new(false),
            left_icon,
            left_text,
            right_icon,
            right_text,
            chev_l,
            chev_r,
            badge,
            badge_text,
            wing,
            physical: layout.notch.physical,
        }
    }

    /// Is any playing-audio content on screen right now (cover art, browser
    /// icon or equaliser)? Decides whether the collapsed island takes clicks
    /// for the quick play/pause toggle.
    pub fn media_showing(&self) -> bool {
        !self.root.isHidden() && (!self.art_box.isHidden() || !self.eq.isHidden())
    }

    /// Is `window_point` over a wing that is showing the playing audio? The
    /// whole wing is the target: the cover art (or browser icon) has the left
    /// wing to itself, and the equaliser has the right one. The rest of the
    /// collapsed island stays click-through.
    pub fn media_hit(&self, window_point: NSPoint) -> bool {
        if !self.media_showing() {
            return false;
        }
        let local = self.root.convertPoint_fromView(window_point, None);
        let size = self.root.frame().size;
        let slack = 4.0;
        let in_band = (0.0..=size.height).contains(&local.y);
        (!self.art_box.isHidden() && local.x < self.wing + slack && in_band)
            || (!self.eq.isHidden() && local.x >= size.width - self.wing - slack && in_band)
    }

    /// Draw `img` as the wing icon through the box's layer contents (layers
    /// draw in every context, offscreen renders included); symbols that need
    /// a tint go through the image view instead.
    fn set_art(&self, img: Option<&objc2_app_kit::NSImage>, fallback: Option<(&str, &str, &objc2_app_kit::NSColor)>) {
        let layer = self.art_box.layer();
        let cg = img.and_then(|i| {
            // SAFETY: null rect / context / hints are the documented defaults.
            unsafe { i.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }
        });
        match (&cg, &layer) {
            (Some(cg), Some(l)) => {
                // SAFETY: a CGImage is a valid CALayer contents object.
                let obj =
                    unsafe { &*(&**cg as *const objc2_core_graphics::CGImage as *const objc2::runtime::AnyObject) };
                unsafe { l.setContents(Some(obj)) };
                l.setContentsGravity(&objc2_foundation::NSString::from_str("resizeAspect"));
                self.art.setHidden(true);
            }
            _ => {
                if let Some(l) = &layer {
                    unsafe { l.setContents(None) };
                }
                if let Some((sym, spoken, tint)) = fallback {
                    self.art.setImage(crate::ui::symbol_image(sym, 12.0, spoken).as_deref());
                    self.art.setContentTintColor(Some(tint));
                    self.art.setHidden(false);
                }
            }
        }
    }

    fn set_equalizer(&self, on: bool, color: &objc2_app_kit::NSColor, animate: bool) {
        use objc2_quartz_core::{CABasicAnimation, CAMediaTiming};
        self.eq.setHidden(!on);
        let cg = color.CGColor();
        for bar in &self.eq_bars {
            bar.setBackgroundColor(Some(&cg));
        }
        let want = on && animate;
        if want == self.eq_animating.replace(want) {
            return;
        }
        let key = objc2_foundation::NSString::from_str("eq");
        for (bar, d) in self.eq_bars.iter().zip(EQ_DURATIONS) {
            if want {
                let anim = CABasicAnimation::animationWithKeyPath(Some(&objc2_foundation::NSString::from_str(
                    "transform.scale.y",
                )));
                let from = objc2_foundation::NSNumber::new_f64(0.3);
                let to = objc2_foundation::NSNumber::new_f64(1.0);
                // SAFETY: NSNumber values for a scalar key path.
                unsafe {
                    anim.setFromValue(Some(&from));
                    anim.setToValue(Some(&to));
                }
                anim.setDuration(d);
                anim.setAutoreverses(true);
                anim.setRepeatCount(f32::INFINITY);
                bar.addAnimation_forKey(&anim, Some(&key));
            } else {
                bar.removeAnimationForKey(&key);
            }
        }
    }

    pub fn update(&self, snap: &Snapshot, p: &Palette, opts: WingOptions) {
        self.update_inner(snap, p, opts);
        self.place();
        // A wing that is dodged (menu bar items beside the notch) is not shown
        // at all: no clipped content, and nothing there can take clicks.
        if opts.mask & WING_LEFT == 0 {
            self.art_box.setHidden(true);
            self.art.setHidden(true);
        }
        if opts.mask & WING_RIGHT == 0 {
            self.set_equalizer(false, &p.success, false);
        }
        // The chevron stands in for a dodged wing that has content: visible
        // only while that wing is actually hidden (not popped out).
        self.chev_l.setHidden(self.physical || opts.chevrons & WING_LEFT == 0 || opts.mask & WING_LEFT != 0);
        self.chev_r.setHidden(self.physical || opts.chevrons & WING_RIGHT == 0 || opts.mask & WING_RIGHT != 0);
    }

    /// Centre each shoulder's content in the shoulder, whatever its width.
    /// A shoulder shows one thing where it is narrow: an icon, or a short
    /// figure. An icon's count turns into a badge on its corner; other text
    /// that doesn't fit beside an icon gives way to it.
    fn place(&self) {
        let size = self.root.frame().size;
        let (w, h, wing) = (size.width, size.height, self.wing);
        let pad = if wing < NARROW { 3.0 } else { 8.0 };
        self.badge.setHidden(true);
        self.place_side(0.0, &self.left_icon, &self.left_text, pad, h, false);
        self.place_side(w - wing, &self.right_icon, &self.right_text, pad, h, true);
        let art = self.art_box.frame().size.width;
        let art_frame = rect((wing - art) / 2.0, (h - art) / 2.0, art, art);
        self.art_box.setFrame(art_frame);
        self.art.setFrame(art_frame);
        let eq = self.eq.frame().size;
        self.eq.setFrame(rect(w - wing + (wing - eq.width) / 2.0, (h - eq.height) / 2.0, eq.width, eq.height));
    }

    fn place_side(&self, x0: f64, icon_v: &NSImageView, text_v: &NSTextField, pad: f64, h: f64, badge: bool) {
        const ICON: f64 = 15.0;
        const GAP: f64 = 3.0;
        let room = self.wing - 2.0 * pad;
        let has_icon = !icon_v.isHidden();
        let mut has_text = !text_v.isHidden();
        // The field draws its text inset a couple of points from each side.
        let mut tw = if has_text { crate::ui::text_width(&text_v.attributedStringValue()).ceil() + 5.0 } else { 0.0 };
        let count = text_v.stringValue().to_string();
        let is_count = !count.is_empty() && count.len() <= 2 && count.chars().all(|c| c.is_ascii_digit());
        let narrow = self.wing < NARROW;
        if has_icon && has_text && (ICON + GAP + tw > room || (narrow && badge && is_count)) {
            text_v.setHidden(true);
            has_text = false;
            // A count rides on the icon's corner instead.
            if badge && is_count {
                self.badge_text.setStringValue(&objc2_foundation::NSString::from_str(&count));
                let bw = if count.len() > 1 { 17.0 } else { 13.0 };
                let ix = x0 + (self.wing - ICON) / 2.0;
                self.badge.setFrame(rect(ix + ICON - 7.0, (h - ICON) / 2.0 - 5.0, bw, 13.0));
                self.badge_text.setFrame(rect(0.0, 1.0, bw, 11.0));
                self.badge.setHidden(false);
            }
        }
        if !has_text {
            tw = 0.0;
        }
        tw = tw.min(room);
        let total = if has_icon { ICON } else { 0.0 } + if has_icon && has_text { GAP } else { 0.0 } + tw;
        let mut x = x0 + (self.wing - total) / 2.0;
        if has_icon {
            icon_v.setFrame(rect(x, (h - ICON) / 2.0 - 1.0, ICON + 2.0, ICON + 2.0));
            x += ICON + GAP;
        }
        if has_text {
            // The field gets the rest of the shoulder, not just its measured
            // width: a tight field truncates text that would fit.
            if has_icon {
                text_v.setAlignment(NSTextAlignment::Left);
                text_v.setFrame(rect(x - 2.0, (h - 15.0) / 2.0, x0 + self.wing - x + 2.0, 15.0));
            } else {
                text_v.setAlignment(NSTextAlignment::Center);
                text_v.setFrame(rect(x0, (h - 15.0) / 2.0, self.wing, 15.0));
            }
        }
    }

    /// Is the chevron for `side` (`WING_LEFT` / `WING_RIGHT`) on screen?
    pub fn chevron_showing(&self, side: u8) -> bool {
        (side & WING_LEFT != 0 && !self.chev_l.isHidden()) || (side & WING_RIGHT != 0 && !self.chev_r.isHidden())
    }

    /// Is `window_point` over one of the wing chevrons? Returns the side's
    /// `WING_LEFT` / `WING_RIGHT` bit; only visible chevrons count.
    pub fn chevron_hit(&self, window_point: NSPoint) -> Option<u8> {
        if self.root.isHidden() {
            return None;
        }
        let local = self.root.convertPoint_fromView(window_point, None);
        let size = self.root.frame().size;
        let in_band = (0.0..=size.height).contains(&local.y);
        if !in_band {
            return None;
        }
        if !self.chev_l.isHidden() && local.x < self.wing + 16.0 {
            return Some(WING_LEFT);
        }
        if !self.chev_r.isHidden() && local.x >= size.width - self.wing - 16.0 {
            return Some(WING_RIGHT);
        }
        None
    }

    fn update_inner(&self, snap: &Snapshot, p: &Palette, opts: WingOptions) {
        // The most recent player track, playing or paused (the controller only
        // lets a paused one through shortly after it was playing).
        let current = snap.media.primary().filter(|_| opts.media);
        let playing = current.filter(|n| n.is_playing());
        // Audio from anything else (a browser tab, a video, a call) gets the
        // same treatment: the app's icon on the left, the equaliser on the right.
        let browser = snap.audio.playing_app().filter(|_| opts.media && playing.is_none());
        // A paused track keeps the wings for a moment so one click resumes it:
        // dimmed cover, static bars. Whatever is actually audible wins.
        let paused = current.filter(|n| !n.is_playing()).filter(|_| browser.is_none());
        let low = snap.battery.as_ref().is_some_and(|b| b.is_low());

        // Recording the screen or a voice memo: a red dot and how long, above
        // everything else.
        if let Some((kind, started)) = crate::capture::active() {
            let secs = (notchforge_core::time::now_ms() - started).max(0) / 1000;
            let text = if self.wing < NARROW && secs >= 60 {
                format!("{}m", secs / 60)
            } else if self.wing < NARROW {
                format!("{secs}s")
            } else {
                format!("{:02}:{:02}", secs / 60, secs % 60)
            };
            self.art.setHidden(true);
            self.art_box.setHidden(true);
            self.set_equalizer(false, &p.success, false);
            let (sym, spoken) = match kind {
                crate::capture::Recording::Screen => ("record.circle.fill", "Recording the screen"),
                crate::capture::Recording::Voice => ("mic.fill", "Recording a voice memo"),
            };
            set_symbol(&self.left_icon, sym, 12.0, spoken);
            self.left_icon.setContentTintColor(Some(&crate::ui::rgba(1.0, 0.27, 0.23, 1.0)));
            self.left_icon.setHidden(false);
            self.left_text.setHidden(true);
            self.right_icon.setHidden(true);
            set_text(&self.right_text, &text);
            set_accessibility_label(&self.right_text, &format!("{spoken}, {text}"));
            self.right_text.setHidden(false);
            return;
        }

        // A running countdown timer: mm:ss, above everything else.
        if let Some(timer) = snap.timer.as_ref() {
            let now = notchforge_core::time::now_ms();
            let remaining = (timer.ends_at_ms - now).max(0);
            let (m, s) = (remaining / 60_000, remaining % 60_000 / 1000);
            let text = if timer.expired {
                "done".to_owned()
            } else if self.wing < NARROW {
                // A narrow shoulder fits minutes, or seconds in the last one.
                if remaining >= 60_000 { format!("{}m", (remaining + 59_999) / 60_000) } else { format!("{s}s") }
            } else {
                format!("{m:02}:{s:02}")
            };
            self.art.setHidden(true);
            self.art_box.setHidden(true);
            self.set_equalizer(false, &p.success, false);
            // A pomodoro's break gets its own colour and symbol.
            let on_break = timer.pomodoro.is_some() && !timer.phase_work;
            set_symbol(&self.left_icon, if on_break { "cup.and.saucer.fill" } else { "timer" }, 12.0, "Timer");
            self.left_icon.setContentTintColor(Some(if on_break { &p.success } else { &p.warning }));
            self.left_icon.setHidden(false);
            self.left_text.setHidden(true);
            self.right_icon.setHidden(true);
            set_text(&self.right_text, &text);
            set_accessibility_label(
                &self.right_text,
                &if timer.expired { "Timer finished".to_owned() } else { format!("Timer, {} remaining", text) },
            );
            self.right_text.setHidden(false);
            return;
        }

        // A running stopwatch: how long it has been going.
        if let Some(sw) = snap.stopwatch.as_ref().filter(|s| s.running()) {
            let secs = sw.elapsed_at(notchforge_core::time::now_ms()) / 1000;
            let text = if self.wing < NARROW {
                if secs >= 3600 {
                    format!("{}h", secs / 3600)
                } else if secs >= 60 {
                    format!("{}m", secs / 60)
                } else {
                    format!("{secs}s")
                }
            } else {
                format!("{:02}:{:02}", secs / 60, secs % 60)
            };
            self.art.setHidden(true);
            self.art_box.setHidden(true);
            self.set_equalizer(false, &p.success, false);
            set_symbol(&self.left_icon, "stopwatch", 12.0, "Stopwatch");
            self.left_icon.setContentTintColor(Some(&p.accent));
            self.left_icon.setHidden(false);
            self.left_text.setHidden(true);
            self.right_icon.setHidden(true);
            set_text(&self.right_text, &text);
            set_accessibility_label(&self.right_text, &format!("Stopwatch, {text}"));
            self.right_text.setHidden(false);
            return;
        }

        // A call in progress: a phone and how long it's been.
        if let Some(call) = snap.active_call.as_ref().filter(|_| snap.calls.is_empty()) {
            self.art.setHidden(true);
            self.art_box.setHidden(true);
            self.set_equalizer(false, &p.success, false);
            set_symbol(&self.left_icon, "phone.fill", 12.0, "On a call");
            self.left_icon.setContentTintColor(Some(&p.success));
            self.left_icon.setHidden(false);
            self.left_text.setHidden(true);
            self.right_icon.setHidden(true);
            let clock = notchforge_core::calls::call_clock(call.started_ms, notchforge_core::time::now_ms());
            set_text(&self.right_text, &clock);
            set_accessibility_label(&self.right_text, &format!("On a call with {} for {clock}", call.caller));
            self.right_text.setHidden(false);
            return;
        }

        // An agent working right now: its name and context use. It renders in
        // whichever wing is free (menu bar items may occupy one side), so the
        // whole message fits in one wing either way.
        if let Some(a) = snap.agent_active.first() {
            self.art.setHidden(true);
            self.art_box.setHidden(true);
            self.set_equalizer(false, &p.success, false);
            let text = match (a.project.as_deref(), a.context_percent) {
                (Some(project), Some(ctx)) => {
                    let name = project.rsplit('/').next().unwrap_or(project);
                    let dirty = if a.dirty { "●" } else { "" };
                    match &a.branch {
                        Some(b) => format!("{name} ⎇{b}{dirty} {ctx}%"),
                        None => format!("{name}{dirty} {ctx}%"),
                    }
                }
                (Some(project), None) => project.rsplit('/').next().unwrap_or(project).to_owned(),
                (None, Some(ctx)) => format!("{} {ctx}%", a.agent.display_name()),
                (None, None) => a.agent.display_name().to_owned(),
            };
            let spoken = format!("{} is working", a.agent.display_name());
            let use_left = opts.mask & WING_LEFT != 0;
            if use_left {
                // Left wing: sparkle icon + the status text.
                set_symbol(&self.left_icon, "sparkles", 12.0, "Agent working");
                self.left_icon.setContentTintColor(Some(&p.accent));
                self.left_icon.setHidden(false);
                set_text(&self.left_text, &text);
                set_accessibility_label(&self.left_text, &spoken);
                self.left_text.setHidden(false);
                self.right_icon.setHidden(true);
                self.right_text.setHidden(true);
            } else if opts.mask & WING_RIGHT != 0 {
                // Right wing only: the status text, or the sparkle where it
                // can't fit.
                self.left_icon.setHidden(true);
                self.left_text.setHidden(true);
                if self.wing < NARROW {
                    super::clear_app_tile(&self.right_icon);
                    set_symbol(&self.right_icon, "sparkles", 12.0, &spoken);
                    self.right_icon.setContentTintColor(Some(&p.accent));
                    self.right_icon.setHidden(false);
                    self.right_text.setHidden(true);
                } else {
                    self.right_icon.setHidden(true);
                    set_text(&self.right_text, &text);
                    set_accessibility_label(&self.right_text, &spoken);
                    self.right_text.setHidden(false);
                }
            } else {
                // Both wings dodged: nothing renders here.
                self.left_icon.setHidden(true);
                self.left_text.setHidden(true);
                self.right_icon.setHidden(true);
                self.right_text.setHidden(true);
            }
            return;
        }

        // A meeting about to start: the countdown until it does. Sits below
        // timers and calls, above the regular wings.
        let now = notchforge_core::time::now_ms();
        if let Some(m) = snap
            .calendar
            .events
            .iter()
            .filter(|e| e.starts_at_ms > now)
            .filter(|e| e.starts_at_ms - now <= 15 * 60_000)
            .min_by_key(|e| e.starts_at_ms)
        {
            self.art.setHidden(true);
            self.art_box.setHidden(true);
            self.set_equalizer(false, &p.success, false);
            set_symbol(&self.left_icon, "calendar.badge.clock", 12.0, "Meeting soon");
            self.left_icon.setContentTintColor(Some(&p.accent));
            self.left_icon.setHidden(false);
            self.left_text.setHidden(true);
            self.right_icon.setHidden(true);
            let mins = (m.starts_at_ms - now + 59_999) / 60_000;
            let soon = if self.wing < NARROW { format!("{mins}m") } else { format!("in {mins}m") };
            set_text(&self.right_text, &soon);
            set_accessibility_label(&self.right_text, &format!("{} in {} minutes", m.title, mins));
            self.right_text.setHidden(false);
            return;
        }

        // Calls moved out of the way: a phone and how many are ringing.
        if !snap.calls.is_empty() {
            self.art.setHidden(true);
            self.art_box.setHidden(true);
            self.set_equalizer(false, &p.success, false);
            set_symbol(&self.left_icon, "phone.fill", 12.0, "Calls ringing");
            self.left_icon.setContentTintColor(Some(&p.success));
            self.left_icon.setHidden(false);
            self.left_text.setHidden(true);
            self.right_icon.setHidden(true);
            set_text(&self.right_text, &snap.calls.len().to_string());
            set_accessibility_label(&self.right_text, &format!("{} calls ringing", snap.calls.len()));
            self.right_text.setHidden(false);
            return;
        }

        // Left wing: low battery, else the playing track's cover, else power / tray.
        let cover = playing.filter(|_| !low).map(|n| {
            let img = n.artwork_path.as_deref().and_then(crate::icons::file_image);
            (n, img)
        });
        let browser_wing = cover.is_none() && browser.filter(|_| !low).is_some();
        let paused_cover = if cover.is_none() && !browser_wing { paused.filter(|_| !low) } else { None };
        if paused_cover.is_some() {
            self.art_box.setAlphaValue(0.55);
        } else {
            self.art_box.setAlphaValue(1.0);
        }
        match &cover {
            Some((n, img)) => {
                let tint = crate::ui::rgb8(n.player.color());
                self.set_art(img.as_deref(), Some(("music.note", "Music", &tint)));
                set_accessibility_label(&self.art_box, &format!("Playing {} by {}", n.title, n.artist));
                self.art_box.setHidden(false);
                self.left_icon.setHidden(true);
                self.left_text.setHidden(true);
            }
            None if browser_wing => {
                let src = browser.expect("checked above");
                let icon = crate::icons::source_icon(src.bundle_id.as_deref(), src.app_path.as_deref());
                self.set_art(icon.as_deref(), Some(("waveform", &src.name, &p.success)));
                set_accessibility_label(&self.art_box, &format!("{} is playing audio", src.name));
                self.art_box.setHidden(false);
                self.left_icon.setHidden(true);
                self.left_text.setHidden(true);
            }
            None => match &paused_cover {
                Some(n) => {
                    let img = n.artwork_path.as_deref().and_then(crate::icons::file_image);
                    let tint = crate::ui::rgb8(n.player.color());
                    self.set_art(img.as_deref(), Some(("music.note", "Music", &tint)));
                    set_accessibility_label(&self.art_box, &format!("Paused {} by {}", n.title, n.artist));
                    self.art_box.setHidden(false);
                    self.left_icon.setHidden(true);
                    self.left_text.setHidden(true);
                }
                None => {
                    self.art.setHidden(true);
                    self.art_box.setHidden(true);
                }
            },
        }
        let left = match &snap.battery {
            _ if cover.is_some() || browser_wing || paused_cover.is_some() => None,
            Some(b) if b.present && b.charging => {
                Some(("bolt.fill", format!("{}%", b.percent), &p.success, "Charging"))
            }
            Some(b) if b.is_low() => Some((b.symbol(), format!("{}%", b.percent), &p.warning, "Battery low")),
            _ if !snap.tray.is_empty() => {
                Some(("tray.full.fill", snap.tray.len().to_string(), &p.secondary, "Files in tray"))
            }
            _ => None,
        };
        match left {
            Some((sym, text, color, spoken)) => {
                set_symbol(&self.left_icon, sym, 12.0, spoken);
                self.left_icon.setContentTintColor(Some(color));
                set_text(&self.left_text, &text);
                self.left_icon.setHidden(false);
                self.left_text.setHidden(false);
                set_accessibility_label(&self.left_text, &format!("{spoken}: {text}"));
            }
            None => {
                self.left_icon.setHidden(true);
                self.left_text.setHidden(true);
            }
        }

        // Right wing: unread notifications (latest app icon + count), else the
        // equaliser while music plays (static and dimmed while paused), else Focus.
        let eq = snap.unread == 0 && (playing.is_some() || browser.is_some() || paused.is_some());
        let eq_color =
            playing.or(paused).map(|n| crate::ui::rgb8(n.player.color())).unwrap_or_else(|| p.success.clone());
        let eq_animates = opts.animate && paused.is_none();
        self.set_equalizer(eq, &eq_color, eq_animates);
        if paused.is_some() && eq {
            self.eq.setAlphaValue(0.55);
            set_accessibility_label(&self.eq, "Music paused");
        } else {
            self.eq.setAlphaValue(1.0);
        }
        if eq {
            self.right_icon.setHidden(true);
            self.right_text.setHidden(true);
        } else if snap.unread > 0 {
            if let Some(latest) = &snap.latest {
                set_app_icon(&self.right_icon, &latest.app);
            }
            set_text(&self.right_text, &snap.unread.min(99).to_string());
            set_accessibility_label(&self.right_text, &format!("{} unread notifications", snap.unread));
            self.right_icon.setHidden(false);
            self.right_text.setHidden(false);
        } else if snap.focus.active == Some(true) {
            super::clear_app_tile(&self.right_icon);
            set_symbol(&self.right_icon, "moon.fill", 12.0, "Focus on");
            self.right_icon.setContentTintColor(Some(&crate::ui::rgba(0.62, 0.52, 1.0, 1.0)));
            self.right_icon.setHidden(false);
            self.right_text.setHidden(true);
        } else {
            self.right_icon.setHidden(true);
            self.right_text.setHidden(true);
        }
    }
}
