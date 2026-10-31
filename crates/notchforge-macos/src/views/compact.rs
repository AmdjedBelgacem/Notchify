//! Hover glance, built to the *Hover · glance* board (520 × 176 island):
//! what's playing (or the latest notification) across two columns, then a
//! weather card and a battery card.
//!
//! ```text
//! ┌──────────────────────[ notch ]──────────────────────┐
//! │ ╭──────────────────────────╮ ╭──────────╮ ╭────────╮ │
//! │ │ ▣  Spotify          ▮▮▮  │ │ ☁        │ │ ▭    🎧 │ │
//! │ │ ▣  Afterglow             │ │ 10°      │ │ 82%    │ │
//! │ │ ▣  Neon Harbor  ━━━──    │ │ Paris…   │ │ 5:12   │ │
//! │ ╰──────────────────────────╯ ╰──────────╯ ╰────────╯ │
//! └──────────────────────────────────────────────────────┘
//! ```

use super::{app_icon_view, battery_spoken, battery_text, set_app_icon, temperature};
use crate::ui::{
    FlippedView, Palette, Weight, attributed, font, label, mono_digits, rect, rgb8, rgba, set_accessibility_label,
    set_gradient, set_symbol, set_text, shape_view, symbol_view, wrapping_label,
};
use notchforge_core::engine::Snapshot;
use notchforge_core::geometry::{LayoutKind, NotchLayout};
use notchforge_core::model::{NotificationKind, Priority};
use notchforge_core::time::{now_ms, relative};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSImageScaling, NSImageView, NSTextField, NSView};
use objc2_foundation::NSPoint;

pub struct CompactView {
    pub root: Retained<FlippedView>,
    mtm: MainThreadMarker,
    // Left (two columns): media or latest notification.
    lead: Retained<FlippedView>,
    lead_wash: Retained<objc2_quartz_core::CAGradientLayer>,
    art: Retained<NSImageView>,
    art_box: Retained<FlippedView>,
    lead_kicker: Retained<NSTextField>,
    lead_title: Retained<NSTextField>,
    lead_sub: Retained<NSTextField>,
    lead_body: Retained<NSTextField>,
    progress_track: Retained<FlippedView>,
    progress_fill: Retained<FlippedView>,
    eq_host: Retained<FlippedView>,
    showing_media: std::cell::Cell<bool>,
    // Weather.
    weather_card: Retained<FlippedView>,
    weather_icon: Retained<NSImageView>,
    weather_temp: Retained<NSTextField>,
    weather_place: Retained<NSTextField>,
    // Battery.
    battery_card: Retained<FlippedView>,
    battery_glyph_host: Retained<FlippedView>,
    accessory_icon: Retained<NSImageView>,
    battery_pct: Retained<NSTextField>,
    battery_detail: Retained<NSTextField>,
}

impl CompactView {
    pub fn new(mtm: MainThreadMarker, layout: &NotchLayout, p: &Palette) -> Self {
        let s = layout.shape(LayoutKind::Compact);
        let nh = layout.notch.rect.height;
        let root = FlippedView::new(mtm, rect(0.0, 0.0, s.width, s.height));
        let pad = 14.0;
        let gap = 8.0;
        let top = nh + 10.0;
        let ch = s.height - top - pad;
        let col = (s.width - 2.0 * pad - 3.0 * gap) / 4.0;
        let white = |a: f64| rgba(1.0, 1.0, 1.0, a);

        // ── lead card (2 columns) ──
        let lw = 2.0 * col + gap;
        let lead = shape_view(mtm, rect(pad, top, lw, ch), &white(0.06), 16.0);
        let lead_wash = objc2_quartz_core::CAGradientLayer::layer();
        lead_wash.setType(&objc2_foundation::NSString::from_str("radial"));
        lead_wash.setFrame(rect(0.0, 0.0, lw, ch));
        lead_wash.setStartPoint(NSPoint::new(0.0, 0.0));
        lead_wash.setEndPoint(NSPoint::new(0.9, 1.2));
        if let Some(l) = lead.layer() {
            l.setMasksToBounds(true);
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&white(0.08).CGColor()));
            l.addSublayer(&lead_wash);
        }
        let art_side = (ch - 24.0).min(72.0);
        let art_box = shape_view(mtm, rect(12.0, (ch - art_side) / 2.0, art_side, art_side), &white(0.08), 11.0);
        if let Some(l) = art_box.layer() {
            l.setMasksToBounds(true);
        }
        let art = NSImageView::new(mtm);
        art.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        art.setFrame(rect(0.0, 0.0, art_side, art_side));
        art_box.addSubview(&art);
        let tx = 12.0 + art_side + 12.0;
        let tw = lw - tx - 12.0;
        let lead_kicker = label(mtm, "", &font(11.0, Weight::Semibold), &p.secondary, rect(tx, 16.0, tw - 20.0, 14.0));
        let lead_title = label(mtm, "", &font(15.0, Weight::Semibold), &p.primary, rect(tx, 33.0, tw, 19.0));
        let lead_sub = label(mtm, "", &font(12.0, Weight::Regular), &p.secondary, rect(tx, 53.0, tw, 16.0));
        let lead_body =
            wrapping_label(mtm, "", &font(11.5, Weight::Regular), &p.secondary, rect(tx, 53.0, tw, 30.0), 2);
        let progress_track = shape_view(mtm, rect(tx, 76.0, tw, 3.0), &white(0.16), 1.5);
        let progress_fill = shape_view(mtm, rect(0.0, 0.0, 0.0, 3.0), &white(0.9), 1.5);
        progress_track.addSubview(&progress_fill);
        let eq_host = FlippedView::new(mtm, rect(lw - 12.0 - 12.0, 17.0, 12.0, 11.0));
        for v in [&*art_box as &NSView, &lead_kicker, &lead_title, &lead_sub, &lead_body, &progress_track, &eq_host] {
            lead.addSubview(v);
        }

        // ── weather (1 column) ──
        let wx = pad + lw + gap;
        let weather_card = FlippedView::new(mtm, rect(wx, top, col, ch));
        weather_card.setWantsLayer(true);
        if let Some(l) = weather_card.layer() {
            l.setCornerRadius(16.0);
            l.setMasksToBounds(true);
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&white(0.1).CGColor()));
            let g = crate::ui::gradient_layer(
                &[(0.353, 0.549, 0.863, 0.30), (0.235, 0.314, 0.549, 0.14)],
                NSPoint::new(0.0, 0.0),
                NSPoint::new(1.0, 1.0),
            );
            g.setFrame(rect(0.0, 0.0, col, ch));
            l.addSublayer(&g);
        }
        let weather_icon =
            symbol_view(mtm, "cloud.sun.fill", 18.0, &p.primary, rect(12.0, 12.0, 24.0, 21.0), "Weather");
        let weather_temp = label(
            mtm,
            "--°",
            &mono_digits(26.0, Weight::Semibold),
            &p.primary,
            rect(12.0, ch - 46.0, col - 18.0, 30.0),
        );
        let weather_place =
            label(mtm, "", &font(11.0, Weight::Regular), &white(0.7), rect(12.0, ch - 19.0, col - 18.0, 14.0));
        for v in [&*weather_icon as &NSView, &weather_temp, &weather_place] {
            weather_card.addSubview(v);
        }

        // ── battery (1 column) ──
        let bx = wx + col + gap;
        let battery_card = shape_view(mtm, rect(bx, top, col, ch), &white(0.06), 16.0);
        if let Some(l) = battery_card.layer() {
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&white(0.08).CGColor()));
        }
        let battery_glyph_host = FlippedView::new(mtm, rect(12.0, 14.0, 28.0, 13.0));
        let accessory_icon =
            symbol_view(mtm, "airpodspro", 12.0, &white(0.6), rect(col - 12.0 - 16.0, 13.0, 16.0, 15.0), "Accessory");
        let battery_pct =
            label(mtm, "--", &mono_digits(26.0, Weight::Semibold), &p.primary, rect(12.0, ch - 46.0, col - 18.0, 30.0));
        let battery_detail =
            label(mtm, "", &font(11.0, Weight::Regular), &white(0.7), rect(12.0, ch - 19.0, col - 18.0, 14.0));
        for v in [&*battery_glyph_host as &NSView, &accessory_icon, &battery_pct, &battery_detail] {
            battery_card.addSubview(v);
        }

        for v in [&lead, &weather_card, &battery_card] {
            root.addSubview(v);
        }
        Self {
            root,
            mtm,
            lead,
            lead_wash,
            art,
            art_box,
            lead_kicker,
            lead_title,
            lead_sub,
            lead_body,
            progress_track,
            progress_fill,
            eq_host,
            showing_media: std::cell::Cell::new(false),
            weather_card,
            weather_icon,
            weather_temp,
            weather_place,
            battery_card,
            battery_glyph_host,
            accessory_icon,
            battery_pct,
            battery_detail,
        }
    }

    /// Is `window_point` on the now-playing card?
    pub fn media_hit(&self, window_point: NSPoint) -> bool {
        if !self.showing_media.get() || self.root.isHidden() {
            return false;
        }
        let local = self.lead.convertPoint_fromView(window_point, None);
        let size = self.lead.frame().size;
        (0.0..=size.width).contains(&local.x) && (0.0..=size.height).contains(&local.y)
    }

    fn set_wash(&self, color: Option<(f64, f64, f64)>) {
        match color {
            Some((r, g, b)) => {
                set_gradient(&self.lead_wash, &[(r, g, b, 0.34), (r, g, b, 0.0)]);
                self.lead_wash.setHidden(false);
            }
            None => self.lead_wash.setHidden(true),
        }
    }

    fn set_equalizer(&self, color: Option<&objc2_app_kit::NSColor>) {
        crate::ui::remove_all_subviews(&self.eq_host);
        if let Some(c) = color {
            let f = self.eq_host.frame();
            self.eq_host.addSubview(&super::media::equalizer_view(
                self.mtm,
                rect(0.0, 0.0, f.size.width, f.size.height),
                c,
            ));
        }
    }

    /// Lead card: what's playing. Returns whether something was shown.
    fn render_media(&self, snap: &Snapshot, p: &Palette) -> bool {
        use super::media::{Featured, featured};
        match featured(&snap.media, &snap.audio) {
            Featured::Player(pl) => {
                let Some(n) = snap.media.player(pl) else { return false };
                let brand = rgb8(pl.color());
                let img = n.artwork_path.as_deref().and_then(crate::icons::file_image);
                match &img {
                    Some(img) => {
                        self.art.setImage(Some(img));
                        self.art.setContentTintColor(None);
                    }
                    None => {
                        self.art.setImage(crate::ui::symbol_image("music.note", 24.0, "Music").as_deref());
                        self.art.setContentTintColor(Some(&brand));
                    }
                }
                let (r, g, b) = pl.color();
                let tint = img.as_deref().and_then(crate::icons::average_color).unwrap_or((
                    r as f64 / 255.0,
                    g as f64 / 255.0,
                    b as f64 / 255.0,
                ));
                self.set_wash(Some(tint));
                self.lead_kicker.setAttributedStringValue(&attributed(
                    pl.display_name(),
                    &font(11.0, Weight::Semibold),
                    &brand,
                ));
                set_text(&self.lead_title, &n.title);
                set_text(&self.lead_sub, if n.artist.is_empty() { &n.album } else { &n.artist });
                self.lead_sub.setHidden(false);
                self.lead_body.setHidden(true);
                let frac = match (n.position_at(now_ms()), n.duration) {
                    (Some(pos), Some(d)) if d > 0.0 => (pos / d).clamp(0.0, 1.0),
                    _ => 0.0,
                };
                let tw = self.progress_track.frame().size.width;
                self.progress_fill.setFrame(rect(0.0, 0.0, tw * frac, 3.0));
                self.progress_track.setHidden(n.duration.is_none());
                self.set_equalizer(n.is_playing().then_some(&*brand));
                set_accessibility_label(
                    &self.lead,
                    &format!("{}: {}, {}. Click to open the player.", pl.display_name(), n.title, n.subtitle()),
                );
                true
            }
            Featured::App { name, bundle_id, app_path } => {
                let icon = crate::icons::source_icon(bundle_id.as_deref(), app_path.as_deref());
                self.art.setImage(icon.as_deref());
                self.art.setContentTintColor(None);
                self.set_wash(None);
                self.lead_kicker.setAttributedStringValue(&attributed(
                    "Playing audio",
                    &font(11.0, Weight::Semibold),
                    &p.success,
                ));
                set_text(&self.lead_title, &name);
                let route = snap
                    .audio
                    .sources
                    .iter()
                    .find(|s| s.name == name)
                    .map(|s| snap.audio.route_label(s))
                    .unwrap_or_default();
                set_text(&self.lead_sub, &route);
                self.lead_sub.setHidden(false);
                self.lead_body.setHidden(true);
                self.progress_track.setHidden(true);
                self.set_equalizer(Some(&p.success));
                set_accessibility_label(&self.lead, &format!("{name} is playing audio through {route}"));
                true
            }
            Featured::Nothing => false,
        }
    }

    /// Lead card: the latest notification, or "All caught up".
    fn render_notification(&self, snap: &Snapshot, p: &Palette) {
        self.set_wash(None);
        self.set_equalizer(None);
        self.progress_track.setHidden(true);
        self.lead_sub.setHidden(true);
        self.lead_body.setHidden(false);
        match &snap.latest {
            Some(n) => {
                let urgent = n.priority >= Priority::High
                    || matches!(n.kind, NotificationKind::NeedsInput | NotificationKind::Error);
                if urgent {
                    self.set_wash(Some((1.0, 0.62, 0.04)));
                }
                self.art.setImage(None);
                crate::ui::remove_all_subviews(&self.art_box);
                let icon = app_icon_view(self.mtm, rect(0.0, 0.0, 1.0, 1.0));
                let side = self.art_box.frame().size.width;
                icon.setFrame(rect(side * 0.18, side * 0.18, side * 0.64, side * 0.64));
                set_app_icon(&icon, &n.app);
                self.art_box.addSubview(&self.art);
                self.art_box.addSubview(&icon);
                let mut kicker = format!("{} · {}", n.app.display_name, relative(n.timestamp_ms, snap.now_ms));
                if snap.unread > 1 {
                    kicker.push_str(&format!(" · {} new", snap.unread));
                }
                let color = if urgent { rgba(1.0, 0.70, 0.25, 1.0) } else { p.secondary.clone() };
                self.lead_kicker.setAttributedStringValue(&attributed(&kicker, &font(11.0, Weight::Semibold), &color));
                set_text(&self.lead_title, &n.title);
                set_text(&self.lead_body, &n.body.replace('\n', " "));
                set_accessibility_label(&self.lead, &n.plain_text());
            }
            None => {
                crate::ui::remove_all_subviews(&self.art_box);
                self.art_box.addSubview(&self.art);
                self.art.setImage(crate::ui::symbol_image("checkmark.circle", 26.0, "All caught up").as_deref());
                self.art.setContentTintColor(Some(&p.tertiary));
                self.lead_kicker.setAttributedStringValue(&attributed(
                    "Notifications",
                    &font(11.0, Weight::Semibold),
                    &p.secondary,
                ));
                set_text(&self.lead_title, "All caught up");
                set_text(
                    &self.lead_body,
                    if snap.paused { "Notifications are paused" } else { "No new notifications" },
                );
                set_accessibility_label(&self.lead, "All caught up. No new notifications.");
            }
        }
    }

    pub fn update(&self, snap: &Snapshot, p: &Palette) {
        // Lead: unread notifications win; otherwise what's playing.
        let media = snap.unread == 0 && {
            crate::ui::remove_all_subviews(&self.art_box);
            self.art_box.addSubview(&self.art);
            self.render_media(snap, p)
        };
        self.showing_media.set(media);
        if !media {
            self.render_notification(snap, p);
        }

        match &snap.weather {
            Some(w) => {
                set_symbol(&self.weather_icon, w.condition.symbol(w.is_day), 18.0, w.condition.label());
                super::multicolor(&self.weather_icon);
                set_text(&self.weather_temp, &temperature(w.temperature));
                set_text(
                    &self.weather_place,
                    &format!("{} · H {} L {}", w.place, temperature(w.high), temperature(w.low)),
                );
                set_accessibility_label(
                    &self.weather_card,
                    &format!("Weather in {}: {}, {}", w.place, temperature(w.temperature), w.condition.label()),
                );
            }
            None => {
                set_text(&self.weather_temp, "--°");
                let hint = match snap.providers.get("weather") {
                    Some(notchforge_core::model::ProviderStatus::NeedsPermission(_)) => "Set a location",
                    Some(notchforge_core::model::ProviderStatus::Disabled) => "Off",
                    _ => "Loading…",
                };
                set_text(&self.weather_place, hint);
            }
        }

        crate::ui::remove_all_subviews(&self.battery_glyph_host);
        match &snap.battery {
            Some(b) => {
                let (pct, detail) = battery_text(b);
                if b.present {
                    self.battery_glyph_host.addSubview(&super::expanded::battery_glyph(
                        self.mtm,
                        rect(0.0, 0.0, 28.0, 13.0),
                        b,
                        p,
                    ));
                }
                set_text(&self.battery_pct, &pct);
                let lowest = snap.devices.iter().filter_map(|d| d.min_level().map(|l| (d, l))).min_by_key(|(_, l)| *l);
                let detail = match lowest {
                    Some((_, l)) if l <= 20 => format!("{detail} · accessory {l}%"),
                    _ => detail,
                };
                set_text(&self.battery_detail, &detail);
                set_accessibility_label(&self.battery_card, &battery_spoken(b));
            }
            None => set_text(&self.battery_pct, "--"),
        }
        match snap.devices.iter().min_by_key(|d| d.min_level().unwrap_or(100)) {
            Some(d) => {
                set_symbol(&self.accessory_icon, d.kind.symbol(), 12.0, &d.name);
                let low = d.min_level().is_some_and(|l| l <= 20);
                let tint = if low { p.warning.clone() } else { rgba(1.0, 1.0, 1.0, 0.6) };
                self.accessory_icon.setContentTintColor(Some(&tint));
                self.accessory_icon.setHidden(false);
            }
            None => self.accessory_icon.setHidden(true),
        }
    }
}
