//! Content shown inside the island, one container per layout.
//!
//! Containers are fixed-size, top-anchored [`FlippedView`]s; the island clips
//! them while its springs animate, so growing reveals content progressively
//! without any per-frame relayout. Views are rebuilt only when the screen
//! geometry or appearance changes.

pub mod collapsed;
pub mod compact;
pub mod expanded;
pub mod peek;

use crate::ui::{FlippedView, Palette, rgb8, symbol_image};
use notchforge_core::apps::AppRef;
use notchforge_core::config::Config;
use notchforge_core::engine::EngineHandle;
use notchforge_core::geometry::{LayoutKind, NotchLayout};
use notchforge_core::model::{BatteryComponent, BatteryStatus, DeviceBattery, TemperatureUnit};
use notchforge_core::time::hours_minutes;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSImageScaling, NSImageView};
use objc2_foundation::NSRect;
use std::sync::Arc;

pub struct Views {
    pub collapsed: collapsed::CollapsedView,
    pub peek: peek::PeekView,
    pub compact: compact::CompactView,
    pub expanded: expanded::ExpandedView,
}

impl Views {
    pub fn new(mtm: MainThreadMarker, layout: &NotchLayout, config: &Arc<Config>, engine: &EngineHandle) -> Self {
        let palette = Palette::new(config.appearance.high_contrast || system_increase_contrast());
        Self {
            collapsed: collapsed::CollapsedView::new(mtm, layout, &palette),
            peek: peek::PeekView::new(mtm, layout, &palette),
            compact: compact::CompactView::new(mtm, layout, &palette),
            expanded: expanded::ExpandedView::new(mtm, layout, palette, config.clone(), engine.clone()),
        }
    }

    pub fn container(&self, kind: LayoutKind) -> &Retained<FlippedView> {
        match kind {
            LayoutKind::Collapsed | LayoutKind::CollapsedWithWings => &self.collapsed.root,
            LayoutKind::Peek => &self.peek.root,
            LayoutKind::Compact => &self.compact.root,
            LayoutKind::Expanded => &self.expanded.root,
        }
    }

    pub fn all(&self) -> [&Retained<FlippedView>; 4] {
        [&self.collapsed.root, &self.peek.root, &self.compact.root, &self.expanded.root]
    }
}

pub fn system_increase_contrast() -> bool {
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast()
}

pub fn system_reduce_motion() -> bool {
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

/// An image view showing the app's real icon, or its tinted SF Symbol.
pub fn app_icon_view(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSImageView> {
    let v = NSImageView::new(mtm);
    v.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    v.setFrame(frame);
    v
}

pub fn set_app_icon(v: &NSImageView, app: &AppRef) {
    match crate::icons::app_icon(app) {
        Some(img) => {
            v.setImage(Some(&img));
            v.setContentTintColor(None);
        }
        None => {
            v.setImage(symbol_image(crate::icons::fallback_symbol(app), 16.0, &app.display_name).as_deref());
            v.setContentTintColor(Some(&rgb8(crate::icons::accent(app))));
        }
    }
    crate::ui::set_accessibility_label(v, &app.display_name);
}

pub fn temperature(t: f32) -> String {
    format!("{:.0}°", t)
}

pub fn unit_suffix(u: TemperatureUnit) -> &'static str {
    u.suffix()
}

/// (headline, detail) for the Mac battery.
pub fn battery_text(b: &BatteryStatus) -> (String, String) {
    if !b.present {
        return ("AC".into(), "Power adapter".into());
    }
    let detail = match (b.charging, b.plugged_in, b.minutes_remaining) {
        (true, _, Some(m)) => format!("Full in {}", hours_minutes(m)),
        (true, _, None) => "Charging".into(),
        (false, true, _) => "Plugged in".into(),
        (false, false, Some(m)) => format!("{} left", hours_minutes(m)),
        (false, false, None) => "On battery".into(),
    };
    (format!("{}%", b.percent), detail)
}

pub fn device_levels(d: &DeviceBattery) -> String {
    d.levels
        .iter()
        .map(|(c, l)| match c {
            BatteryComponent::Main => format!("{l}%"),
            other => format!("{} {l}%", other.short_label()),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Spoken summary of the battery for VoiceOver.
pub fn battery_spoken(b: &BatteryStatus) -> String {
    let (head, detail) = battery_text(b);
    format!("Battery {head}, {detail}")
}
