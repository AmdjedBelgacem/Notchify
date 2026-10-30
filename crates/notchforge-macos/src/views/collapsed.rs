//! Collapsed state: the island hugs the notch; optional "wings" either side
//! show live signals (charging, low battery, tray contents, unread messages).

use super::{app_icon_view, set_app_icon};
use crate::ui::{
    FlippedView, Palette, Weight, label, mono_digits, rect, set_accessibility_label, set_symbol, set_text, symbol_view,
};
use notchforge_core::engine::Snapshot;
use notchforge_core::geometry::{LayoutKind, NotchLayout, WING_WIDTH};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSImageView, NSTextAlignment, NSTextField};

pub struct CollapsedView {
    pub root: Retained<FlippedView>,
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
        for v in [&*left_icon as &objc2_app_kit::NSView, &left_text, &right_icon, &right_text] {
            root.addSubview(v);
        }
        Self { root, left_icon, left_text, right_icon, right_text }
    }

    pub fn update(&self, snap: &Snapshot, p: &Palette) {
        // Left wing: power first (charging / low), then tray.
        let left = match &snap.battery {
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

        // Right wing: unread notifications (latest app icon + count), else Focus.
        if snap.unread > 0 {
            if let Some(latest) = &snap.latest {
                set_app_icon(&self.right_icon, &latest.app);
            }
            set_text(&self.right_text, &snap.unread.min(99).to_string());
            set_accessibility_label(&self.right_text, &format!("{} unread notifications", snap.unread));
            self.right_icon.setHidden(false);
            self.right_text.setHidden(false);
        } else if snap.focus.active == Some(true) {
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
