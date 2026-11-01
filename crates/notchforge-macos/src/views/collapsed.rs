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
use notchforge_core::geometry::{LayoutKind, NotchLayout, WING_WIDTH};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSImageView, NSTextAlignment, NSTextField};

/// Per-render options from the controller.
#[derive(Debug, Clone, Copy)]
pub struct WingOptions {
    /// Show media (cover + equaliser) in the wings.
    pub media: bool,
    /// Animate the equaliser (off with Reduce Motion).
    pub animate: bool,
}

const EQ_HEIGHTS: [f64; 4] = [7.0, 12.0, 9.0, 11.0];
const EQ_DURATIONS: [f64; 4] = [0.42, 0.55, 0.38, 0.5];
const EQ_BAR: f64 = 2.5;
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
}

impl CollapsedView {
    pub fn new(mtm: MainThreadMarker, layout: &NotchLayout, p: &Palette) -> Self {
        let s = layout.shape(LayoutKind::CollapsedWithWings);
        let (w, h) = (s.width, s.height);
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        let icon = 15.0;
        let iy = (h - icon) / 2.0;
        let ty = (h - 15.0) / 2.0;
        let f = mono_digits(11.5, Weight::Semibold);
        let left_icon = symbol_view(mtm, "bolt.fill", 12.0, &p.success, rect(12.0, iy, icon, icon), "Battery");
        let left_text = label(mtm, "", &f, &p.primary, rect(29.0, ty, WING_WIDTH - 31.0, 15.0));
        let right_icon = app_icon_view(mtm, rect(w - WING_WIDTH + 10.0, iy - 1.0, icon + 2.0, icon + 2.0));
        let right_text = label(mtm, "", &f, &p.primary, rect(w - WING_WIDTH + 30.0, ty, WING_WIDTH - 42.0, 15.0));
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

        for v in [&*left_icon as &objc2_app_kit::NSView, &left_text, &right_icon, &right_text, &art_box, &art, &eq] {
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
        }
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
        let playing = snap.media.primary().filter(|n| opts.media && n.is_playing());
        // Audio from anything else (a browser tab, a video, a call) gets the
        // same treatment: the app's icon on the left, the equaliser on the right.
        let browser = snap.audio.playing_app().filter(|_| opts.media && playing.is_none());
        let low = snap.battery.as_ref().is_some_and(|b| b.is_low());

        // Left wing: low battery, else the playing track's cover, else power / tray.
        let cover = playing.filter(|_| !low).map(|n| {
            let img = n.artwork_path.as_deref().and_then(crate::icons::file_image);
            (n, img)
        });
        match &cover {
            Some((n, img)) => {
                let tint = crate::ui::rgb8(n.player.color());
                self.set_art(img.as_deref(), Some(("music.note", "Music", &tint)));
                set_accessibility_label(&self.art_box, &format!("Playing {} by {}", n.title, n.artist));
                self.art_box.setHidden(false);
                self.left_icon.setHidden(true);
                self.left_text.setHidden(true);
            }
            None => match browser.filter(|_| !low) {
                Some(src) => {
                    let icon = crate::icons::source_icon(src.bundle_id.as_deref(), src.app_path.as_deref());
                    self.set_art(icon.as_deref(), Some(("waveform", &src.name, &p.success)));
                    set_accessibility_label(&self.art_box, &format!("{} is playing audio", src.name));
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
        let browser_wing = cover.is_none() && browser.is_some() && !low;
        let left = match &snap.battery {
            _ if cover.is_some() || browser_wing => None,
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
        // equaliser while music plays, else Focus.
        let eq = snap.unread == 0 && (playing.is_some() || browser.is_some());
        let eq_color = playing.map(|n| crate::ui::rgb8(n.player.color())).unwrap_or_else(|| p.success.clone());
        self.set_equalizer(eq, &eq_color, opts.animate);
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
