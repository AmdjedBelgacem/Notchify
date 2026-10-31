//! Hover state: weather · battery · latest notification (or what's playing)
//! at a glance.

use super::{app_icon_view, battery_spoken, battery_text, device_levels, set_app_icon, temperature};
use crate::ui::{
    FlippedView, Palette, Weight, font, label, make_card, mono_digits, rect, set_accessibility_label, set_symbol,
    set_text, symbol_view, wrapping_label,
};
use notchforge_core::engine::Snapshot;
use notchforge_core::geometry::{LayoutKind, NotchLayout};
use notchforge_core::time::relative;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSImageView, NSTextField, NSView};

pub struct CompactView {
    pub root: Retained<FlippedView>,
    weather_card: Retained<FlippedView>,
    weather_icon: Retained<NSImageView>,
    weather_temp: Retained<NSTextField>,
    weather_place: Retained<NSTextField>,
    battery_card: Retained<FlippedView>,
    battery_icon: Retained<NSImageView>,
    battery_pct: Retained<NSTextField>,
    battery_detail: Retained<NSTextField>,
    device_line: Retained<NSTextField>,
    notif_card: Retained<FlippedView>,
    notif_icon: Retained<NSImageView>,
    notif_header: Retained<NSTextField>,
    notif_title: Retained<NSTextField>,
    notif_body: Retained<NSTextField>,
    media_card: Retained<FlippedView>,
    media_art: Retained<NSImageView>,
    media_badge: Retained<NSTextField>,
    media_title: Retained<NSTextField>,
    media_subtitle: Retained<NSTextField>,
    media_state: Retained<NSImageView>,
}

impl CompactView {
    pub fn new(mtm: MainThreadMarker, layout: &NotchLayout, p: &Palette) -> Self {
        let s = layout.shape(LayoutKind::Compact);
        let nh = layout.notch.rect.height;
        let root = FlippedView::new(mtm, rect(0.0, 0.0, s.width, s.height));
        let pad = 14.0;
        let gap = 8.0;
        let top = nh + 6.0;
        let ch = s.height - top - pad + 4.0;
        let col = (s.width - 2.0 * pad - 2.0 * gap) / 4.0;

        // Weather card (1 column).
        let weather_card = FlippedView::new(mtm, rect(pad, top, col, ch));
        make_card(&weather_card, &p.card, 14.0);
        let weather_icon =
            symbol_view(mtm, "cloud.sun.fill", 20.0, &p.primary, rect(10.0, 10.0, 24.0, 24.0), "Weather");
        let weather_temp =
            label(mtm, "--°", &mono_digits(22.0, Weight::Semibold), &p.primary, rect(10.0, 36.0, col - 16.0, 28.0));
        let weather_place =
            label(mtm, "Weather", &font(11.0, Weight::Regular), &p.secondary, rect(10.0, 64.0, col - 16.0, 14.0));
        for v in [&*weather_icon as &NSView, &weather_temp, &weather_place] {
            weather_card.addSubview(v);
        }

        // Battery card (1 column).
        let battery_card = FlippedView::new(mtm, rect(pad + col + gap, top, col, ch));
        make_card(&battery_card, &p.card, 14.0);
        let battery_icon =
            symbol_view(mtm, "battery.100percent", 18.0, &p.primary, rect(10.0, 10.0, 30.0, 24.0), "Battery");
        let battery_pct =
            label(mtm, "--", &mono_digits(22.0, Weight::Semibold), &p.primary, rect(10.0, 36.0, col - 16.0, 28.0));
        let battery_detail =
            label(mtm, "", &font(11.0, Weight::Regular), &p.secondary, rect(10.0, 64.0, col - 16.0, 14.0));
        let device_line = label(mtm, "", &font(10.0, Weight::Regular), &p.tertiary, rect(44.0, 14.0, col - 50.0, 14.0));
        for v in [&*battery_icon as &NSView, &battery_pct, &battery_detail, &device_line] {
            battery_card.addSubview(v);
        }

        // Latest notification (2 columns).
        let nx = pad + 2.0 * (col + gap);
        let nw = s.width - nx - pad;
        let notif_card = FlippedView::new(mtm, rect(nx, top, nw, ch));
        make_card(&notif_card, &p.card, 14.0);
        let notif_icon = app_icon_view(mtm, rect(10.0, 10.0, 20.0, 20.0));
        let notif_header = label(mtm, "", &font(11.0, Weight::Medium), &p.secondary, rect(36.0, 12.0, nw - 46.0, 14.0));
        let notif_title = label(mtm, "", &font(13.0, Weight::Semibold), &p.primary, rect(10.0, 34.0, nw - 20.0, 17.0));
        let notif_body =
            wrapping_label(mtm, "", &font(11.5, Weight::Regular), &p.secondary, rect(10.0, 52.0, nw - 20.0, 30.0), 2);
        for v in [&*notif_icon as &NSView, &notif_header, &notif_title, &notif_body] {
            notif_card.addSubview(v);
        }

        // Now playing (same slot as the notification card; shown when nothing is unread).
        let media_card = FlippedView::new(mtm, rect(nx, top, nw, ch));
        make_card(&media_card, &p.card, 14.0);
        let art_side = (ch - 20.0).min(60.0);
        let art_box = FlippedView::new(mtm, rect(10.0, (ch - art_side) / 2.0, art_side, art_side));
        make_card(&art_box, &p.card, 8.0);
        if let Some(l) = art_box.layer() {
            l.setMasksToBounds(true);
        }
        let media_art = NSImageView::new(mtm);
        media_art.setImageScaling(objc2_app_kit::NSImageScaling::ScaleProportionallyUpOrDown);
        media_art.setFrame(rect(0.0, 0.0, art_side, art_side));
        art_box.addSubview(&media_art);
        let mx = 10.0 + art_side + 10.0;
        let mw = nw - mx - 30.0;
        let media_badge = label(mtm, "", &font(10.5, Weight::Semibold), &p.secondary, rect(mx, 12.0, mw, 14.0));
        let media_title = label(mtm, "", &font(13.0, Weight::Semibold), &p.primary, rect(mx, 30.0, mw + 20.0, 17.0));
        let media_subtitle =
            label(mtm, "", &font(11.5, Weight::Regular), &p.secondary, rect(mx, 49.0, mw + 20.0, 15.0));
        let media_state = symbol_view(mtm, "waveform", 12.0, &p.success, rect(nw - 26.0, 11.0, 16.0, 16.0), "Playing");
        for v in [&*art_box as &NSView, &media_badge, &media_title, &media_subtitle, &media_state] {
            media_card.addSubview(v);
        }
        media_card.setHidden(true);

        for v in [&weather_card, &battery_card, &notif_card, &media_card] {
            root.addSubview(v);
        }
        Self {
            root,
            weather_card,
            weather_icon,
            weather_temp,
            weather_place,
            battery_card,
            battery_icon,
            battery_pct,
            battery_detail,
            device_line,
            notif_card,
            notif_icon,
            notif_header,
            notif_title,
            notif_body,
            media_card,
            media_art,
            media_badge,
            media_title,
            media_subtitle,
            media_state,
        }
    }

    /// Is `window_point` on the visible now-playing card?
    pub fn media_hit(&self, window_point: objc2_foundation::NSPoint) -> bool {
        if self.media_card.isHidden() || self.root.isHidden() {
            return false;
        }
        let local = self.media_card.convertPoint_fromView(window_point, None);
        let size = self.media_card.frame().size;
        (0.0..=size.width).contains(&local.x) && (0.0..=size.height).contains(&local.y)
    }

    /// Show what's playing instead of "All caught up". Returns whether shown.
    fn render_media(&self, snap: &Snapshot, p: &Palette) -> bool {
        use super::media::{Featured, featured};
        let show = match featured(&snap.media, &snap.audio) {
            Featured::Player(pl) => {
                let Some(n) = snap.media.player(pl) else { return false };
                let img = n.artwork_path.as_deref().and_then(crate::icons::file_image);
                match img {
                    Some(img) => {
                        self.media_art.setImage(Some(&img));
                        self.media_art.setContentTintColor(None);
                    }
                    None => {
                        self.media_art.setImage(crate::ui::symbol_image("music.note", 20.0, "Music").as_deref());
                        self.media_art.setContentTintColor(Some(&crate::ui::rgb8(pl.color())));
                    }
                }
                self.media_badge.setTextColor(Some(&crate::ui::rgb8(pl.color())));
                set_text(&self.media_badge, pl.display_name());
                set_text(&self.media_title, &n.title);
                set_text(&self.media_subtitle, &n.subtitle());
                let playing = n.is_playing();
                set_symbol(
                    &self.media_state,
                    if playing { "waveform" } else { "pause.fill" },
                    12.0,
                    if playing { "Playing" } else { "Paused" },
                );
                self.media_state.setContentTintColor(Some(if playing { &p.success } else { &p.tertiary }));
                set_accessibility_label(
                    &self.media_card,
                    &format!("{}: {}, {}. Click to open the player.", pl.display_name(), n.title, n.subtitle()),
                );
                true
            }
            Featured::App { name, bundle_id, app_path } => {
                let icon = crate::icons::source_icon(bundle_id.as_deref(), app_path.as_deref());
                self.media_art.setImage(icon.as_deref());
                self.media_art.setContentTintColor(None);
                self.media_badge.setTextColor(Some(&p.secondary));
                set_text(&self.media_badge, "Playing audio");
                set_text(&self.media_title, &name);
                let route = snap
                    .audio
                    .sources
                    .iter()
                    .find(|s| s.name == name)
                    .map(|s| snap.audio.route_label(s))
                    .unwrap_or_default();
                set_text(&self.media_subtitle, &route);
                set_symbol(&self.media_state, "waveform", 12.0, "Playing");
                self.media_state.setContentTintColor(Some(&p.success));
                set_accessibility_label(&self.media_card, &format!("{name} is playing audio through {route}"));
                true
            }
            Featured::Nothing => false,
        };
        self.media_card.setHidden(!show);
        show
    }

    pub fn update(&self, snap: &Snapshot, p: &Palette) {
        match &snap.weather {
            Some(w) => {
                set_symbol(&self.weather_icon, w.condition.symbol(w.is_day), 20.0, w.condition.label());
                set_text(&self.weather_temp, &temperature(w.temperature));
                set_text(&self.weather_place, &w.place);
                set_accessibility_label(
                    &self.weather_card,
                    &format!("Weather in {}: {}, {}", w.place, temperature(w.temperature), w.condition.label()),
                );
            }
            None => {
                set_text(&self.weather_temp, "--°");
                let status = snap.providers.get("weather");
                let hint = match status {
                    Some(notchforge_core::model::ProviderStatus::NeedsPermission(_)) => "Set a location",
                    Some(notchforge_core::model::ProviderStatus::Disabled) => "Off",
                    _ => "Loading…",
                };
                set_text(&self.weather_place, hint);
            }
        }

        match &snap.battery {
            Some(b) => {
                let (pct, detail) = battery_text(b);
                set_symbol(&self.battery_icon, b.symbol(), 18.0, "Battery");
                self.battery_icon.setContentTintColor(Some(if b.charging {
                    &p.success
                } else if b.is_low() {
                    &p.warning
                } else {
                    &p.primary
                }));
                set_text(&self.battery_pct, &pct);
                set_text(&self.battery_detail, &detail);
                set_accessibility_label(&self.battery_card, &battery_spoken(b));
            }
            None => set_text(&self.battery_pct, "--"),
        }
        let dev = snap
            .devices
            .iter()
            .min_by_key(|d| d.min_level().unwrap_or(100))
            .map(|d| format!("{} {}", d.name, device_levels(d)))
            .unwrap_or_default();
        set_text(&self.device_line, &dev);

        self.notif_card.setHidden(false);
        self.media_card.setHidden(true);
        if snap.unread == 0 && self.render_media(snap, p) {
            self.notif_card.setHidden(true);
            return;
        }
        match &snap.latest {
            Some(n) => {
                set_app_icon(&self.notif_icon, &n.app);
                let mut header = format!("{} · {}", n.app.display_name, relative(n.timestamp_ms, snap.now_ms));
                if snap.unread > 1 {
                    header.push_str(&format!(" · {} new", snap.unread));
                }
                set_text(&self.notif_header, &header);
                set_text(&self.notif_title, &n.title);
                set_text(&self.notif_body, &n.body);
                self.notif_icon.setHidden(false);
                set_accessibility_label(&self.notif_card, &n.plain_text());
            }
            None => {
                self.notif_icon.setHidden(true);
                set_text(&self.notif_header, "");
                set_text(&self.notif_title, "All caught up");
                set_text(&self.notif_body, if snap.paused { "Notifications paused" } else { "No new notifications" });
            }
        }
    }
}
