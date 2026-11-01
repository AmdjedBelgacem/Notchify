//! Peek: a banner that drops out of the notch when a notification arrives
//! while collapsed, built to the *Peek · agent needs input* board (440 × 96).
//!
//! ```text
//! ┌──────────────────[ notch ]──────────────────┐
//! │ ░glow░ ╭──╮ ● Claude Code needs your input  │
//! │        │✳ │ Allow running cargo test? (Open)│
//! └─────────────────────────────────────────────┘
//! ```
//!
//! Urgent notifications (needs input, errors, high priority) get an orange
//! wash and an orange summary line; others a quiet grey one.

use super::{app_icon_view, set_app_icon};
use crate::ui::{
    FlippedView, Palette, Weight, attributed, font, label, pill_button, rect, rgba, set_accessibility_label,
    set_gradient, set_text, shape_view, without_animation,
};
use notchforge_core::geometry::{LayoutKind, NotchLayout};
use notchforge_core::model::{Notification, NotificationKind, Priority};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSImageView, NSTextField};
use objc2_foundation::NSPoint;
use std::cell::RefCell;

pub struct PeekView {
    pub root: Retained<FlippedView>,
    glow: Retained<objc2_quartz_core::CAGradientLayer>,
    tile: Retained<FlippedView>,
    icon: Retained<NSImageView>,
    dot: Retained<FlippedView>,
    summary: Retained<NSTextField>,
    title: Retained<NSTextField>,
    open: Retained<NSButton>,
    _target: Retained<crate::ui::ActionTarget>,
    current: RefCell<Option<Notification>>,
}

/// Open a notification's deep link, else its app.
pub fn open_notification(n: &Notification) {
    let opened = n.url.as_deref().is_some_and(crate::sharing::open_url);
    if !opened {
        if let Some(b) = n.app.bundle_id.as_deref() {
            crate::sharing::open_app(b);
        }
    }
}

fn is_urgent(n: &Notification) -> bool {
    n.priority >= Priority::High || matches!(n.kind, NotificationKind::NeedsInput | NotificationKind::Error)
}

impl PeekView {
    pub fn new(mtm: MainThreadMarker, layout: &NotchLayout, p: &Palette) -> Self {
        let s = layout.shape(LayoutKind::Peek);
        let nh = layout.notch.rect.height;
        let (w, h) = (s.width, s.height);
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        root.setWantsLayer(true);
        let glow = objc2_quartz_core::CAGradientLayer::layer();
        glow.setType(&objc2_foundation::NSString::from_str("radial"));
        glow.setFrame(rect(0.0, 0.0, w, h));
        glow.setStartPoint(NSPoint::new(0.0, 1.0));
        glow.setEndPoint(NSPoint::new(0.42, 1.0 - 0.84));
        set_gradient(&glow, &[(1.0, 0.62, 0.04, 0.22), (1.0, 0.62, 0.04, 0.0)]);
        if let Some(l) = root.layer() {
            l.addSublayer(&glow);
        }

        // Content row: 8 pt below the notch, 36 pt tall (the board's top: 40 px).
        let cy = nh + 26.0;
        let tile = shape_view(mtm, rect(16.0, cy - 18.0, 36.0, 36.0), &rgba(1.0, 1.0, 1.0, 0.08), 9.0);
        let icon = app_icon_view(mtm, rect(2.0, 2.0, 32.0, 32.0));
        tile.addSubview(&icon);
        // Board: 12 pt padding either side of the label.
        let open_font = font(12.5, Weight::Semibold);
        let ow = (crate::ui::text_width(&attributed("Open", &open_font, &p.primary)) + 24.0).ceil();
        let tx = 64.0;
        let tw = w - tx - ow - 28.0;
        let dot = shape_view(mtm, rect(tx, cy - 13.0, 6.0, 6.0), &p.warning, 3.0);
        let summary =
            label(mtm, "", &font(11.5, Weight::Semibold), &p.secondary, rect(tx + 12.0, cy - 18.0, tw - 12.0, 15.0));
        let title = label(mtm, "", &font(14.0, Weight::Semibold), &p.primary, rect(tx, cy - 1.0, tw, 18.0));
        let target = crate::ui::ActionTarget::new(mtm, |_| {});
        let open = pill_button(
            mtm,
            "Open",
            None,
            &open_font,
            &objc2_app_kit::NSColor::blackColor(),
            &objc2_app_kit::NSColor::whiteColor(),
            14.0,
            &target,
            rect(w - 16.0 - ow, cy - 14.0, ow, 28.0),
        );
        for v in [&*tile as &objc2_app_kit::NSView, &dot, &summary, &title, &open] {
            root.addSubview(v);
        }
        Self { root, glow, tile, icon, dot, summary, title, open, _target: target, current: RefCell::new(None) }
    }

    pub fn show(&self, n: &Notification, p: &Palette) {
        // Without a real app icon the icon view draws its own brand tile
        // (see `set_app_icon`); the outer tile only frames real icons.
        let branded = crate::icons::app_icon(&n.app).is_none();
        if let Some(l) = self.tile.layer() {
            let c = if branded { rgba(0.0, 0.0, 0.0, 0.0) } else { rgba(1.0, 1.0, 1.0, 0.08) };
            without_animation(|| l.setBackgroundColor(Some(&c.CGColor())));
        }
        self.icon.setFrame(if branded { rect(0.0, 0.0, 36.0, 36.0) } else { rect(2.0, 2.0, 32.0, 32.0) });
        set_app_icon(&self.icon, &n.app);
        let urgent = is_urgent(n);
        let summary = match n.kind {
            NotificationKind::NeedsInput => format!("{} needs your input", n.app.display_name),
            NotificationKind::TaskFinished => format!("{} finished", n.app.display_name),
            NotificationKind::Error => format!("{} hit an error", n.app.display_name),
            _ => n.app.display_name.clone(),
        };
        let color = if urgent { rgba(1.0, 0.70, 0.25, 1.0) } else { p.secondary.clone() };
        self.summary.setAttributedStringValue(&attributed(&summary, &font(11.5, Weight::Semibold), &color));
        self.dot.setHidden(!urgent);
        let shift = if urgent { 0.0 } else { -12.0 };
        let f = self.summary.frame();
        let base_x = self.dot.frame().origin.x + 12.0 - crate::ui::LABEL_INSET;
        self.summary.setFrameOrigin(NSPoint::new(base_x + shift, f.origin.y));
        without_animation(|| self.glow.setHidden(!urgent));
        let mut title = n.title.clone();
        if n.group_count > 1 {
            title.push_str(&format!("  ·  {}", n.group_count));
        }
        if title.trim().is_empty() {
            title = n.body.replace('\n', " ");
        }
        set_text(&self.title, &title);
        set_accessibility_label(&self.root, &format!("New notification. {}", n.plain_text()));
        *self.current.borrow_mut() = Some(n.clone());
    }

    /// Is `window_point` on the Open button?
    pub fn open_hit(&self, window_point: NSPoint) -> bool {
        if self.root.isHidden() {
            return false;
        }
        let local = self.open.convertPoint_fromView(window_point, None);
        let size = self.open.frame().size;
        (0.0..=size.width).contains(&local.x) && (0.0..=size.height).contains(&local.y)
    }

    /// Open the notification shown in the peek.
    pub fn open_current(&self) {
        if let Some(n) = self.current.borrow().as_ref() {
            open_notification(n);
        }
    }
}
