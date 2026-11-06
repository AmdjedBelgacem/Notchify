//! Incoming calls, built to the *Incoming calls* board of the NotchForge design
//! canvas: the first call large with round buttons, any others stacked below
//! it. Clicking a waiting call brings it to the top.

use super::{app_icon_view, set_app_icon};
use crate::app::with_controller;
use crate::ui::{
    FlippedView, Palette, Targets, Weight, click_overlay, font, label, make_card, rect, remove_all_subviews, rgba,
    set_accessibility_label, shape_view, symbol_button,
};
use notchforge_core::calls::{ActiveCall, Call, CallKind, call_clock};
use notchforge_core::geometry::{CALL_ROW_STEP, LayoutKind, MAX_VISIBLE_CALLS, NotchLayout};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSColor;
use std::hash::{Hash, Hasher};

/// What a call's buttons ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallAction {
    Answer,
    Decline,
    Promote,
}

fn green() -> Retained<NSColor> {
    rgba(0.141, 0.541, 0.239, 1.0)
}

fn red() -> Retained<NSColor> {
    rgba(0.851, 0.176, 0.125, 1.0)
}

/// A steady colour for a caller's initials, dark enough for white text.
pub fn avatar_color(name: &str) -> Retained<NSColor> {
    const COLORS: [(f64, f64, f64); 6] = [
        (0.71, 0.28, 0.54),
        (0.43, 0.29, 0.76),
        (0.17, 0.42, 0.69),
        (0.10, 0.48, 0.45),
        (0.70, 0.36, 0.13),
        (0.62, 0.25, 0.25),
    ];
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    let (r, g, b) = COLORS[(h.finish() % COLORS.len() as u64) as usize];
    rgba(r, g, b, 1.0)
}

fn answer_symbol(kind: CallKind) -> &'static str {
    match kind {
        CallKind::Video | CallKind::Meeting => "video.fill",
        CallKind::Audio => "phone.fill",
    }
}

pub struct CallView {
    pub root: Retained<FlippedView>,
    mtm: MainThreadMarker,
    p: Palette,
    nh: f64,
    targets: Targets,
    content: Retained<FlippedView>,
    /// The running time, while a call in progress is showing.
    clock: std::cell::RefCell<Option<Retained<objc2_app_kit::NSTextField>>>,
    /// Keeps the Hide button's action alive.
    _fixed: Targets,
}

impl CallView {
    pub fn new(mtm: MainThreadMarker, layout: &NotchLayout, p: &Palette) -> Self {
        let s = layout.shape(LayoutKind::Incoming(MAX_VISIBLE_CALLS as u8));
        let nh = layout.notch.rect.height;
        let root = FlippedView::new(mtm, rect(0.0, 0.0, s.width, s.height));
        let content = FlippedView::new(mtm, rect(0.0, 0.0, s.width, s.height));
        root.addSubview(&content);
        let fixed = Targets::default();
        let t = fixed.add(mtm, |_| {
            with_controller(|c| c.hide_calls());
        });
        let hide = symbol_button(
            mtm,
            "chevron.up",
            11.0,
            "Hide until it stops ringing",
            &p.primary,
            &t,
            rect(s.width - 14.0 - 26.0, ((nh - 24.0) / 2.0).max(2.0), 26.0, 24.0),
        );
        make_card(&hide, &rgba(1.0, 1.0, 1.0, 0.14), 12.0);
        root.addSubview(&hide);
        Self {
            root,
            mtm,
            p: p.clone(),
            nh,
            targets: Targets::default(),
            content,
            clock: Default::default(),
            _fixed: fixed,
        }
    }

    fn button(
        &self,
        (sym, size): (&str, f64),
        tip: &str,
        color: &NSColor,
        frame: objc2_foundation::NSRect,
        id: Call,
        action: CallAction,
    ) -> Retained<objc2_app_kit::NSButton> {
        let t = self.targets.add(self.mtm, move |_| {
            with_controller(|c| c.call_action(&id, action));
        });
        let b = symbol_button(self.mtm, sym, size, tip, &NSColor::whiteColor(), &t, frame);
        make_card(&b, color, frame.size.width / 2.0);
        b
    }

    /// A call in progress: who it's with, how long it's been, and a way to end it.
    pub fn update_active(&self, call: &ActiveCall, now_ms: i64) {
        self.targets.clear();
        remove_all_subviews(&self.content);
        let mtm = self.mtm;
        let w = self.root.frame().size.width;
        let y = self.nh + 4.0;
        let side = 60.0;
        let disc = shape_view(mtm, rect(16.0, y + 6.0, side, side), &avatar_color(&call.caller), side / 2.0);
        let initials = label(
            mtm,
            &call.initials(),
            &font(22.0, Weight::Semibold),
            &NSColor::whiteColor(),
            rect(0.0, 16.0, side, 28.0),
        );
        initials.setAlignment(objc2_app_kit::NSTextAlignment::Center);
        disc.addSubview(&initials);
        self.content.addSubview(&disc);
        let tx = 16.0 + side + 14.0;
        let btn = 52.0;
        let tw = w - 16.0 - btn - 12.0 - tx;
        self.content.addSubview(&label(
            mtm,
            &format!("On call · {}", call.source),
            &font(12.5, Weight::Regular),
            &rgba(0.435, 0.89, 0.549, 1.0),
            rect(tx, y + 8.0, tw, 17.0),
        ));
        self.content.addSubview(&label(
            mtm,
            &call.caller,
            &font(20.0, Weight::Semibold),
            &self.p.primary,
            rect(tx, y + 25.0, tw, 26.0),
        ));
        let clock = label(
            mtm,
            &call_clock(call.started_ms, now_ms),
            &crate::ui::mono_digits(13.0, Weight::Medium),
            &rgba(1.0, 1.0, 1.0, 0.84),
            rect(tx, y + 51.0, tw, 17.0),
        );
        self.content.addSubview(&clock);
        *self.clock.borrow_mut() = Some(clock);
        let t = self.targets.add(mtm, |_| {
            with_controller(|c| c.hang_up());
        });
        let hang_up = symbol_button(
            mtm,
            "phone.down.fill",
            20.0,
            &format!("Hang up on {}", call.caller),
            &NSColor::whiteColor(),
            &t,
            rect(w - 16.0 - btn, y + 10.0, btn, btn),
        );
        make_card(&hang_up, &red(), btn / 2.0);
        self.content.addSubview(&hang_up);
    }

    /// Move the running time on.
    pub fn set_clock(&self, text: &str) {
        if let Some(c) = &*self.clock.borrow() {
            crate::ui::set_text(c, text);
        }
    }

    pub fn update(&self, calls: &[Call], animate: bool) {
        self.targets.clear();
        *self.clock.borrow_mut() = None;
        remove_all_subviews(&self.content);
        let Some(top) = calls.first() else { return };
        let mtm = self.mtm;
        let p = &self.p;
        let w = self.root.frame().size.width;
        if calls.len() > MAX_VISIBLE_CALLS {
            let more = label(
                mtm,
                &format!("+{} more ringing", calls.len() - MAX_VISIBLE_CALLS),
                &font(12.0, Weight::Semibold),
                &p.primary,
                rect(16.0, ((self.nh - 16.0) / 2.0).max(4.0), 140.0, 16.0),
            );
            self.content.addSubview(&more);
        }

        // The call on top.
        let y = self.nh + 4.0;
        let side = 60.0;
        let ay = y + 6.0;
        let color = avatar_color(&top.caller);
        let host = FlippedView::new(mtm, rect(16.0, ay, side, side));
        host.setWantsLayer(true);
        let pulse = shape_view(mtm, rect(0.0, 0.0, side, side), &color, side / 2.0);
        if animate {
            pulse_ring(&pulse, side);
        }
        host.addSubview(&pulse);
        let disc = shape_view(mtm, rect(0.0, 0.0, side, side), &color, side / 2.0);
        let initials = label(
            mtm,
            &top.initials(),
            &font(22.0, Weight::Semibold),
            &NSColor::whiteColor(),
            rect(0.0, 16.0, side, 28.0),
        );
        initials.setAlignment(objc2_app_kit::NSTextAlignment::Center);
        disc.addSubview(&initials);
        host.addSubview(&disc);
        let badge = app_icon_view(mtm, rect(side - 21.0, side - 21.0, 24.0, 24.0));
        set_app_icon(&badge, &top.app);
        host.addSubview(&badge);
        self.content.addSubview(&host);

        let tx = 16.0 + side + 14.0;
        let btn = 52.0;
        let buttons_x = w - 16.0 - btn - 12.0 - btn;
        let tw = buttons_x - 12.0 - tx;
        self.content.addSubview(&label(
            mtm,
            &format!("{} · {}", if top.answering { "Picking up" } else { top.kind.label() }, top.source),
            &font(12.5, Weight::Regular),
            &rgba(1.0, 1.0, 1.0, 0.8),
            rect(tx, y + 15.0, tw, 17.0),
        ));
        self.content.addSubview(&label(
            mtm,
            &top.caller,
            &font(22.0, Weight::Semibold),
            &p.primary,
            rect(tx, y + 32.0, tw, 28.0),
        ));
        let by = y + 10.0;
        self.content.addSubview(&self.button(
            ("phone.down.fill", 20.0),
            &format!("Decline {}", top.caller),
            &red(),
            rect(buttons_x, by, btn, btn),
            top.clone(),
            CallAction::Decline,
        ));
        self.content.addSubview(&self.button(
            (answer_symbol(top.kind), 20.0),
            &format!("Answer {}", top.caller),
            &green(),
            rect(buttons_x + btn + 12.0, by, btn, btn),
            top.clone(),
            CallAction::Answer,
        ));

        // The calls waiting behind it.
        let row_w = w - 24.0;
        for (i, c) in calls.iter().skip(1).take(MAX_VISIBLE_CALLS - 1).enumerate() {
            let ry = self.nh + 84.0 + i as f64 * CALL_ROW_STEP;
            let row = shape_view(mtm, rect(12.0, ry, row_w, 48.0), &rgba(1.0, 1.0, 1.0, 0.1), 16.0);
            if let Some(l) = row.layer() {
                l.setBorderWidth(0.5);
                l.setBorderColor(Some(&rgba(1.0, 1.0, 1.0, 0.18).CGColor()));
            }
            let disc = shape_view(mtm, rect(6.0, 7.0, 34.0, 34.0), &avatar_color(&c.caller), 17.0);
            let il = label(
                mtm,
                &c.initials(),
                &font(13.0, Weight::Semibold),
                &NSColor::whiteColor(),
                rect(0.0, 9.0, 34.0, 16.0),
            );
            il.setAlignment(objc2_app_kit::NSTextAlignment::Center);
            disc.addSubview(&il);
            row.addSubview(&disc);
            let text_w = row_w - 50.0 - 8.0 - 34.0 - 8.0 - 34.0 - 10.0;
            row.addSubview(&label(
                mtm,
                &c.caller,
                &font(14.0, Weight::Semibold),
                &p.primary,
                rect(50.0, 7.0, text_w, 18.0),
            ));
            row.addSubview(&label(
                mtm,
                &format!("{} · {}", c.kind.label(), c.source),
                &font(12.0, Weight::Regular),
                &rgba(1.0, 1.0, 1.0, 0.78),
                rect(50.0, 25.0, text_w, 15.0),
            ));
            let promote = c.clone();
            let t = self.targets.add(mtm, move |_| {
                with_controller(|ctl| ctl.call_action(&promote, CallAction::Promote));
            });
            row.addSubview(&click_overlay(
                mtm,
                &t,
                rect(0.0, 0.0, row_w - 84.0, 48.0),
                &format!("Show {} first", c.caller),
            ));
            row.addSubview(&self.button(
                ("phone.down.fill", 13.0),
                &format!("Decline {}", c.caller),
                &red(),
                rect(row_w - 8.0 - 34.0 - 8.0 - 34.0, 7.0, 34.0, 34.0),
                c.clone(),
                CallAction::Decline,
            ));
            row.addSubview(&self.button(
                (answer_symbol(c.kind), 13.0),
                &format!("Answer {}", c.caller),
                &green(),
                rect(row_w - 8.0 - 34.0, 7.0, 34.0, 34.0),
                c.clone(),
                CallAction::Answer,
            ));
            self.content.addSubview(&row);
        }
        set_accessibility_label(&self.content, &format!("{} from {} on {}", top.kind.label(), top.caller, top.source));
    }
}

/// A ring that keeps growing out from the avatar and fading.
fn pulse_ring(view: &FlippedView, side: f64) {
    use objc2_foundation::{NSNumber, NSString};
    use objc2_quartz_core::{CABasicAnimation, CAMediaTiming};
    let Some(l) = view.layer() else { return };
    // Grow from the middle rather than the corner.
    l.setAnchorPoint(objc2_foundation::NSPoint::new(0.5, 0.5));
    l.setPosition(objc2_foundation::NSPoint::new(side / 2.0, side / 2.0));
    let animate = |key: &str, from: f64, to: f64| {
        let a = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str(key)));
        let (from, to) = (NSNumber::new_f64(from), NSNumber::new_f64(to));
        // SAFETY: NSNumber values for a scalar key path.
        unsafe {
            a.setFromValue(Some(&from));
            a.setToValue(Some(&to));
        }
        a.setDuration(1.4);
        a.setRepeatCount(f32::INFINITY);
        l.addAnimation_forKey(&a, Some(&NSString::from_str(key)));
    };
    animate("transform.scale", 1.0, 1.32);
    animate("opacity", 0.55, 0.0);
}
