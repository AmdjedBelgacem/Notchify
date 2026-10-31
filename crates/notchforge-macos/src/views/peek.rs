//! Peek: a compact banner that drops out of the notch when a notification
//! arrives while collapsed.

use super::{app_icon_view, set_app_icon};
use crate::ui::{FlippedView, Palette, Weight, font, label, rect, set_accessibility_label, set_text};
use notchforge_core::geometry::{LayoutKind, NotchLayout};
use notchforge_core::model::{Notification, NotificationKind};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSImageView, NSTextField};

pub struct PeekView {
    pub root: Retained<FlippedView>,
    icon: Retained<NSImageView>,
    title: Retained<NSTextField>,
    body: Retained<NSTextField>,
}

impl PeekView {
    pub fn new(mtm: MainThreadMarker, layout: &NotchLayout, p: &Palette) -> Self {
        let s = layout.shape(LayoutKind::Peek);
        let nh = layout.notch.rect.height;
        let root = FlippedView::new(mtm, rect(0.0, 0.0, s.width, s.height));
        let icon = app_icon_view(mtm, rect(16.0, nh + 10.0, 28.0, 28.0));
        let tw = s.width - 16.0 - 56.0;
        let title = label(mtm, "", &font(13.0, Weight::Semibold), &p.primary, rect(54.0, nh + 7.0, tw, 17.0));
        let body = label(mtm, "", &font(12.0, Weight::Regular), &p.secondary, rect(54.0, nh + 25.0, tw, 16.0));
        root.addSubview(&icon);
        root.addSubview(&title);
        root.addSubview(&body);
        Self { root, icon, title, body }
    }

    pub fn show(&self, n: &Notification, p: &Palette) {
        set_app_icon(&self.icon, &n.app);
        let mut title = n.title.clone();
        if n.group_count > 1 {
            title.push_str(&format!("  ·  {}", n.group_count));
        }
        set_text(&self.title, &title);
        let body = if n.body.is_empty() { n.app.display_name.clone() } else { n.body.replace('\n', " ") };
        set_text(&self.body, &body);
        let color = match n.kind {
            NotificationKind::Error => &p.warning,
            _ => &p.primary,
        };
        self.title.setTextColor(Some(color));
        set_accessibility_label(&self.root, &format!("New notification. {}", n.plain_text()));
    }
}
