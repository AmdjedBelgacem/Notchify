//! Expanded (pinned) panel, built to the *Pinned · Overview* board of the
//! NotchForge design canvas (720 × 372 island). Two pages share the top row:
//! **Overview** (this file) and **Now Playing** ([`super::media`]).
//!
//! ```text
//! ┌(▦|♪) ☁ 10° Paris ───────[ notch ]──────────────────── 82% ▭ ┐
//! │ Notifications 4   (Clear) │ Tray 3         │ ╭ ☁ 10°  Partly cloudy ╮│
//! │ ╭ ✳ Claude Code · Needs… ╮│ ╭────────────╮ │ │ Now 22 23 00 01      ││
//! │ │ Allow running tests?   ││ │ 📄    🖼    │ │ ╰──────────────────────╯│
//! │ ╰────────────────────────╯│ │ 📁          │ │ 🎧 AirPods Pro  80 · 78 │
//! │ ╭ ▸ Cursor · Finished    ╮│ ╰────────────╯ │ ☀ ━━━━━━━━━━──────     │
//! │ ╰────────────────────────╯│  ⦿  ▭  ⧉  🗑   │ 🔈 ━━━━━━━━────────    │
//! │ …                         │                │ (▭ 1512 × 982        ) │
//! │                           │                │ [☾] [🔕] [⚙] [⌃]       │
//! └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! Notification rows open on click; Open, Copy and Dismiss are on the
//! right-click menu, as in Notification Center.

use super::media::MediaPage;
use super::{app_icon_view, set_app_icon, temperature};
use crate::app::with_controller;
use crate::controls::{brightness, display_modes, volume};
use crate::panel::TrayItemView;
use crate::ui::{
    FlippedView, Palette, Targets, ThinSlider, Weight, attributed_parts, click_overlay, font, label, make_card,
    mono_digits, pill_button, rect, remove_all_subviews, rgba, rich_label, set_accessibility_label, shape_view,
    style_pill, symbol_button, symbol_view, wrapping_label,
};
use notchforge_core::config::Config;
use notchforge_core::engine::{Command, EngineHandle, Snapshot};
use notchforge_core::geometry::{LayoutKind, NotchLayout};
use notchforge_core::model::{BatteryComponent, BatteryStatus, DisplayInfo, Notification, NotificationKind, Priority};
use notchforge_core::time::{relative, relative_spoken};
use notchforge_core::tray::format_bytes;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSButton, NSColor, NSFont, NSImageView, NSMenu, NSMenuItem, NSScrollView, NSScrollerStyle, NSTextAlignment,
    NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::Arc;

const X0: f64 = 20.0;
const GAP: f64 = 14.0;
const ROW_GAP: f64 = 6.0;
const MAX_ROWS: usize = 40;
const TILE_H: f64 = 92.0;

type TrayAction = Box<dyn Fn(Vec<PathBuf>, &EngineHandle)>;
type Action = Box<dyn Fn()>;
type Run<'a> = (&'a str, &'a NSFont, &'a NSColor);

/// Pages of the expanded panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Media,
}

pub struct ExpandedView {
    pub root: Retained<FlippedView>,
    overview: Retained<FlippedView>,
    pub media: MediaPage,
    tab: Cell<Tab>,
    tab_buttons: Vec<Retained<NSButton>>,
    /// The panel is expanded (drives the music page's ticker).
    visible: Cell<bool>,
    mtm: MainThreadMarker,
    palette: Palette,
    engine: EngineHandle,
    config: RefCell<Arc<Config>>,
    /// Keeps the static controls' action targets alive.
    _targets: Targets,
    row_targets: Targets,

    weather_top: Retained<FlippedView>,
    battery_top: Retained<FlippedView>,

    notif_header: Retained<NSTextField>,
    notif_scroll: Retained<NSScrollView>,
    notif_doc: Retained<FlippedView>,
    notif_width: f64,

    tray_header: Retained<NSTextField>,
    tray_box: Retained<FlippedView>,
    tray_scroll: Retained<NSScrollView>,
    tray_doc: Retained<FlippedView>,
    tray_empty: Retained<FlippedView>,
    tray_width: f64,
    tray_paths: RefCell<Vec<PathBuf>>,

    weather_card: Retained<FlippedView>,
    devices: Retained<FlippedView>,
    status_width: f64,

    brightness_row: Retained<FlippedView>,
    brightness: Retained<ThinSlider>,
    volume_row: Retained<FlippedView>,
    volume: Retained<ThinSlider>,
    resolution: Retained<NSButton>,
    display: RefCell<Option<DisplayInfo>>,
    display_id: Cell<u32>,
    focus_button: Retained<NSButton>,
    pause_button: Retained<NSButton>,

    last_rendered: Cell<u64>,
}

fn scroll_view(mtm: MainThreadMarker, frame: NSRect, doc: &NSView) -> Retained<NSScrollView> {
    let s = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    s.setDrawsBackground(false);
    s.setHasVerticalScroller(true);
    s.setAutohidesScrollers(true);
    s.setScrollerStyle(NSScrollerStyle::Overlay);
    s.setDocumentView(Some(doc));
    s
}

fn white(a: f64) -> Retained<NSColor> {
    rgba(1.0, 1.0, 1.0, a)
}

/// The menu-bar style battery glyph: outline, fill and nub.
pub fn battery_glyph(mtm: MainThreadMarker, frame: NSRect, b: &BatteryStatus, p: &Palette) -> Retained<FlippedView> {
    let v = FlippedView::new(mtm, frame);
    let (w, h) = (frame.size.width - 3.0, frame.size.height);
    let outline = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
    outline.setWantsLayer(true);
    if let Some(l) = outline.layer() {
        l.setCornerRadius(3.0);
        l.setBorderWidth(1.2);
        l.setBorderColor(Some(&white(0.45).CGColor()));
    }
    // Green as on the design board; orange when low.
    let color = if b.is_low() && !b.charging { p.warning.clone() } else { p.success.clone() };
    let fw = ((w - 4.0) * b.percent as f64 / 100.0).max(1.5);
    v.addSubview(&outline);
    v.addSubview(&shape_view(mtm, rect(2.0, 2.0, fw, h - 4.0), &color, 1.6));
    v.addSubview(&shape_view(mtm, rect(w + 1.0, h / 2.0 - 2.0, 2.0, 4.0), &white(0.45), 1.0));
    v
}

/// Compact levels ("80 · 78 · 18%"), each flagged when low.
pub fn device_parts(levels: &[(BatteryComponent, u8)]) -> Vec<(String, bool)> {
    let n = levels.len();
    levels
        .iter()
        .enumerate()
        .map(|(i, (_, l))| (if i + 1 == n { format!("{l}%") } else { l.to_string() }, *l <= 20))
        .collect()
}

fn runs<'a>(parts: &'a [(String, Retained<NSColor>)], f: &'a NSFont) -> Vec<Run<'a>> {
    parts.iter().map(|(t, c)| (t.as_str(), f, &**c)).collect()
}

impl ExpandedView {
    pub fn new(
        mtm: MainThreadMarker,
        layout: &NotchLayout,
        palette: Palette,
        config: Arc<Config>,
        engine: EngineHandle,
    ) -> Self {
        let p = &palette;
        let s = layout.shape(LayoutKind::Expanded);
        let (w, h) = (s.width, s.height);
        let nh = layout.notch.rect.height;
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        let targets = Targets::default();
        let ty = (nh - 22.0) / 2.0;

        // ── top row: page switch + weather (left), battery (right) ──
        let side = (w - layout.notch.rect.width) / 2.0;
        let weather_top = FlippedView::new(mtm, rect(86.0, ty, (side - 96.0).max(40.0), 22.0));
        let battery_top = FlippedView::new(mtm, rect(w - 18.0 - 110.0, ty, 110.0, 22.0));
        root.addSubview(&weather_top);
        root.addSubview(&battery_top);

        // Overview content lives in its own container so the music page can
        // replace it; both share the root's coordinate space.
        let overview = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        root.addSubview(&overview);

        let body_y = nh + 12.0;
        let body_h = h - body_y - 18.0;
        let c1w = (w * 0.403).floor();
        let c2w = (w * 0.236).floor();
        let c1x = X0;
        let c2x = c1x + c1w + GAP;
        let c3x = c2x + c2w + GAP;
        let c3w = w - 20.0 - c3x;

        // ── notifications ──
        let notif_header = label(
            mtm,
            "Notifications",
            &font(13.0, Weight::Semibold),
            &p.primary,
            rect(c1x + 2.0, body_y + 2.0, c1w - 70.0, 18.0),
        );
        overview.addSubview(&notif_header);
        let eng = engine.clone();
        let t = targets.add(mtm, move |_| eng.send(Command::ClearNotifications));
        overview.addSubview(&pill_button(
            mtm,
            "Clear",
            None,
            &font(11.5, Weight::Semibold),
            &white(0.7),
            &white(0.08),
            11.0,
            &t,
            rect(c1x + c1w - 48.0, body_y, 48.0, 22.0),
        ));
        let notif_doc = FlippedView::new(mtm, rect(0.0, 0.0, c1w, body_h - 30.0));
        let notif_scroll = scroll_view(mtm, rect(c1x, body_y + 30.0, c1w, body_h - 30.0), &notif_doc);
        overview.addSubview(&notif_scroll);

        // ── tray ──
        let tray_header = label(
            mtm,
            "Tray",
            &font(13.0, Weight::Semibold),
            &p.primary,
            rect(c2x + 2.0, body_y + 2.0, c2w - 4.0, 18.0),
        );
        overview.addSubview(&tray_header);
        let box_h = body_h - 30.0 - 38.0;
        let tray_box = shape_view(mtm, rect(c2x, body_y + 30.0, c2w, box_h), &white(0.06), 16.0);
        if let Some(l) = tray_box.layer() {
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&white(0.08).CGColor()));
        }
        let tray_doc = FlippedView::new(mtm, rect(0.0, 0.0, c2w, box_h));
        let tray_scroll = scroll_view(mtm, rect(0.0, 0.0, c2w, box_h), &tray_doc);
        tray_box.addSubview(&tray_scroll);
        let tray_empty = FlippedView::new(mtm, rect(0.0, 0.0, c2w, box_h));
        tray_empty.addSubview(&symbol_view(
            mtm,
            "tray.and.arrow.down",
            26.0,
            &p.tertiary,
            rect((c2w - 34.0) / 2.0, box_h / 2.0 - 32.0, 34.0, 30.0),
            "Drop files",
        ));
        let hint = wrapping_label(
            mtm,
            "Drop files here to hold them",
            &font(11.5, Weight::Regular),
            &p.tertiary,
            rect(12.0, box_h / 2.0 + 4.0, c2w - 24.0, 32.0),
            2,
        );
        hint.setAlignment(NSTextAlignment::Center);
        tray_empty.addSubview(&hint);
        tray_box.addSubview(&tray_empty);
        overview.addSubview(&tray_box);

        let by = body_y + 30.0 + box_h + 8.0;
        let actions: [(&str, &str, TrayAction); 4] = [
            (
                "dot.radiowaves.left.and.right",
                "AirDrop tray files",
                Box::new(|paths, _| {
                    crate::sharing::airdrop(&paths);
                }),
            ),
            ("folder", "Reveal tray files in Finder", Box::new(|paths, _| crate::sharing::reveal(&paths))),
            ("doc.on.doc", "Copy file paths", Box::new(|paths, _| crate::sharing::copy_paths(&paths))),
            ("trash", "Clear tray", Box::new(|_, eng| eng.send(Command::TrayClear))),
        ];
        let step = c2w / actions.len() as f64;
        for (i, (sym, tip, action)) in actions.into_iter().enumerate() {
            let eng = engine.clone();
            let t = targets.add(mtm, move |_| {
                let paths = with_controller(|c| c.tray_paths()).unwrap_or_default();
                action(paths, &eng);
            });
            overview.addSubview(&symbol_button(
                mtm,
                sym,
                14.0,
                tip,
                &white(0.7),
                &t,
                rect(c2x + i as f64 * step + (step - 32.0) / 2.0, by, 32.0, 28.0),
            ));
        }

        // ── status & controls ──
        let weather_card = FlippedView::new(mtm, rect(c3x, body_y, c3w, 104.0));
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
            g.setFrame(rect(0.0, 0.0, c3w, 104.0));
            l.addSublayer(&g);
        }
        overview.addSubview(&weather_card);

        let devices = FlippedView::new(mtm, rect(c3x, body_y + 112.0, c3w, 48.0));
        overview.addSubview(&devices);

        let slider_row = |sym: &str, tip: &str, y: f64, set: fn(f64)| {
            let row = FlippedView::new(mtm, rect(c3x, y, c3w, 20.0));
            row.addSubview(&symbol_view(mtm, sym, 11.0, &white(0.6), rect(0.0, 3.0, 14.0, 14.0), tip));
            let s = ThinSlider::new(mtm, rect(23.0, 2.0, c3w - 23.0, 16.0), false, &white(0.95));
            set_accessibility_label(&s, tip);
            s.set_on_change(set);
            row.addSubview(&s);
            overview.addSubview(&row);
            (row, s)
        };
        let (brightness_row, brightness_slider) =
            slider_row("sun.max.fill", "Display brightness", body_y + 166.0, |v| {
                brightness::set(v as f32);
            });
        let (volume_row, volume_slider) = slider_row("speaker.wave.1.fill", "Output volume", body_y + 190.0, |v| {
            volume::set(v as f32);
        });

        let t = targets.add(mtm, move |sender: Option<&AnyObject>| {
            if let Some(b) = sender.and_then(|s| s.downcast_ref::<NSButton>()) {
                let b = objc2::Message::retain(b);
                with_controller(|c| c.show_resolution_menu(&b));
            }
        });
        let resolution = pill_button(
            mtm,
            "Resolution",
            Some("display"),
            &font(12.0, Weight::Medium),
            &p.primary,
            &white(0.08),
            8.0,
            &t,
            rect(c3x, body_y + 220.0, c3w, 26.0),
        );
        resolution.setAlignment(NSTextAlignment::Left);
        resolution.addSubview(&symbol_view(
            mtm,
            "chevron.up.chevron.down",
            9.0,
            &p.secondary,
            rect(c3w - 22.0, 7.0, 12.0, 12.0),
            "",
        ));
        set_accessibility_label(&resolution, "Display resolution");
        overview.addSubview(&resolution);

        let buttons: [(&str, &str, Action); 4] = [
            (
                "moon.fill",
                "Toggle Focus",
                Box::new(|| {
                    with_controller(|c| c.toggle_focus());
                }),
            ),
            (
                "bell.slash",
                "Pause notifications",
                Box::new(|| {
                    with_controller(|c| c.toggle_pause());
                }),
            ),
            ("gearshape", "Settings", Box::new(crate::app::show_settings)),
            (
                "chevron.up",
                "Collapse",
                Box::new(|| {
                    with_controller(|c| c.collapse());
                }),
            ),
        ];
        let bw = (c3w - 3.0 * 6.0) / 4.0;
        let mut made = Vec::new();
        for (i, (sym, tip, action)) in buttons.into_iter().enumerate() {
            let t = targets.add(mtm, move |_| action());
            let b = symbol_button(
                mtm,
                sym,
                13.0,
                tip,
                &white(0.8),
                &t,
                rect(c3x + i as f64 * (bw + 6.0), body_y + 256.0, bw, 30.0),
            );
            make_card(&b, &white(0.08), 9.0);
            overview.addSubview(&b);
            made.push(b);
        }
        let focus_button = made[0].clone();
        let pause_button = made[1].clone();

        // ── music page + tab buttons (added last so they sit on top) ──
        let media = MediaPage::new(mtm, w, h, nh + 8.0, p);
        media.root.setHidden(true);
        root.addSubview(&media.root);
        let track = shape_view(mtm, rect(14.0, ty - 1.0, 62.0, 24.0), &white(0.08), 9.0);
        root.addSubview(&track);
        let tabs: [(&str, &str, Tab); 2] =
            [("square.grid.2x2.fill", "Overview", Tab::Overview), ("music.note", "Now Playing", Tab::Media)];
        let tab_buttons: Vec<Retained<NSButton>> = tabs
            .iter()
            .enumerate()
            .map(|(i, (sym, tip, tab))| {
                let tab = *tab;
                let t = targets.add(mtm, move |_| {
                    with_controller(|c| c.show_expanded_tab(tab));
                });
                let b = symbol_button(
                    mtm,
                    sym,
                    11.0,
                    tip,
                    &p.tertiary,
                    &t,
                    rect(16.0 + i as f64 * 30.0, ty + 1.0, 28.0, 20.0),
                );
                root.addSubview(&b);
                b
            })
            .collect();

        Self {
            root,
            overview,
            media,
            tab: Cell::new(Tab::Overview),
            tab_buttons,
            visible: Cell::new(false),
            mtm,
            palette,
            engine,
            config: RefCell::new(config),
            _targets: targets,
            row_targets: Targets::default(),
            weather_top,
            battery_top,
            notif_header,
            notif_scroll,
            notif_doc,
            notif_width: c1w,
            tray_header,
            tray_box,
            tray_scroll,
            tray_doc,
            tray_empty,
            tray_width: c2w,
            tray_paths: RefCell::new(Vec::new()),
            weather_card,
            devices,
            status_width: c3w,
            brightness_row,
            brightness: brightness_slider,
            volume_row,
            volume: volume_slider,
            resolution,
            display: RefCell::new(None),
            display_id: Cell::new(layout.screen.id),
            focus_button,
            pause_button,
            last_rendered: Cell::new(0),
        }
    }

    pub fn tab(&self) -> Tab {
        self.tab.get()
    }

    /// Switch pages. Opening the music page asks the providers for fresh data.
    pub fn show_tab(&self, tab: Tab) {
        let previous = self.tab.replace(tab);
        self.overview.setHidden(tab != Tab::Overview);
        self.media.root.setHidden(tab != Tab::Media);
        for (b, t) in self.tab_buttons.iter().zip([Tab::Overview, Tab::Media]) {
            let on = t == tab;
            b.setContentTintColor(Some(if on { &self.palette.primary } else { &self.palette.tertiary }));
            // Selected segment: a lighter pill inside the shared track.
            make_card(b, &white(if on { 0.18 } else { 0.0 }), 7.0);
        }
        self.media.set_active(self.visible.get() && tab == Tab::Media);
        if tab == Tab::Media && previous != Tab::Media {
            crate::media::send(notchforge_core::provider::ProviderCommand::MediaChanged(None));
            crate::media::send(notchforge_core::provider::ProviderCommand::ReloadPlaylists);
        }
    }

    /// The panel opened or closed.
    pub fn set_visible(&self, visible: bool) {
        self.visible.set(visible);
        self.media.set_active(visible && self.tab.get() == Tab::Media);
        if visible && self.tab.get() == Tab::Media {
            crate::media::send(notchforge_core::provider::ProviderCommand::MediaChanged(None));
        }
    }

    pub fn set_config(&self, config: Arc<Config>) {
        *self.config.borrow_mut() = config;
    }

    pub fn tray_paths(&self) -> Vec<PathBuf> {
        self.tray_paths.borrow().clone()
    }

    /// Highlight the tray while files are dragged over the notch.
    pub fn set_drop_highlight(&self, on: bool) {
        if let Some(layer) = self.tray_box.layer() {
            if on {
                layer.setBorderWidth(2.0);
                layer.setBorderColor(Some(&self.palette.accent.CGColor()));
            } else {
                layer.setBorderWidth(0.5);
                layer.setBorderColor(Some(&white(0.08).CGColor()));
            }
        }
    }

    /// Re-render dynamic content. Skips work if this snapshot was rendered.
    pub fn update(&self, snap: &Snapshot) {
        if self.last_rendered.replace(snap.generation) == snap.generation {
            return;
        }
        self.row_targets.clear();
        self.render_top(snap);
        self.media.update(snap);
        self.render_notifications(snap);
        self.render_tray(snap);
        self.render_weather(snap);
        self.render_devices(snap);

        let p = &self.palette;
        let focus_on = snap.focus.active == Some(true);
        let focus_bg = if focus_on { rgba(0.368, 0.361, 0.902, 0.85) } else { white(0.08) };
        make_card(&self.focus_button, &focus_bg, 9.0);
        let dim = white(0.8);
        self.focus_button.setContentTintColor(Some(if focus_on { &p.primary } else { &dim }));
        let pause_bg = if snap.paused { rgba(1.0, 0.62, 0.04, 0.85) } else { white(0.08) };
        make_card(&self.pause_button, &pause_bg, 9.0);
        self.pause_button.setContentTintColor(Some(if snap.paused { &p.primary } else { &dim }));
        set_accessibility_label(
            &self.pause_button,
            if snap.paused { "Resume notifications" } else { "Pause notifications" },
        );
    }

    fn render_top(&self, snap: &Snapshot) {
        let p = &self.palette;
        let mtm = self.mtm;
        remove_all_subviews(&self.weather_top);
        if let Some(w) = &snap.weather {
            let icon = symbol_view(
                mtm,
                w.condition.symbol(w.is_day),
                13.0,
                &p.primary,
                rect(0.0, 3.0, 18.0, 16.0),
                w.condition.label(),
            );
            crate::views::multicolor(&icon);
            self.weather_top.addSubview(&icon);
            let temp = temperature(w.temperature);
            let place = format!("  {}", w.place);
            let (fs, fm) = (mono_digits(13.0, Weight::Semibold), font(13.0, Weight::Medium));
            let text = attributed_parts(&[(&temp, &fs, &p.primary), (&place, &fm, &p.secondary)]);
            let tw = self.weather_top.frame().size.width - 23.0;
            self.weather_top.addSubview(&rich_label(mtm, &text, rect(23.0, 2.0, tw, 18.0)));
        }
        remove_all_subviews(&self.battery_top);
        if let Some(b) = snap.battery.as_ref().filter(|b| b.present) {
            let bw = self.battery_top.frame().size.width;
            let l = label(
                mtm,
                &format!("{}%", b.percent),
                &mono_digits(13.0, Weight::Semibold),
                &p.primary,
                rect(0.0, 2.0, bw - 33.0, 18.0),
            );
            l.setAlignment(NSTextAlignment::Right);
            self.battery_top.addSubview(&l);
            self.battery_top.addSubview(&battery_glyph(mtm, rect(bw - 26.0, 5.0, 26.0, 12.0), b, p));
            set_accessibility_label(&self.battery_top, &super::battery_spoken(b));
        }
    }

    fn render_weather(&self, snap: &Snapshot) {
        let p = &self.palette;
        let mtm = self.mtm;
        let wc = self.status_width;
        remove_all_subviews(&self.weather_card);
        let Some(w) = &snap.weather else {
            let icon = symbol_view(mtm, "cloud.sun.fill", 20.0, &p.secondary, rect(12.0, 12.0, 26.0, 22.0), "Weather");
            self.weather_card.addSubview(&icon);
            let temp = label(mtm, "--°", &mono_digits(26.0, Weight::Semibold), &p.primary, rect(44.0, 8.0, 80.0, 30.0));
            self.weather_card.addSubview(&temp);
            let hint = match snap.providers.get("weather") {
                Some(notchforge_core::model::ProviderStatus::NeedsPermission(_)) => "Set a location in Settings",
                Some(notchforge_core::model::ProviderStatus::Disabled) => "Weather is off",
                _ => "Weather unavailable",
            };
            let l = label(mtm, hint, &font(11.5, Weight::Regular), &p.secondary, rect(12.0, 52.0, wc - 24.0, 15.0));
            self.weather_card.addSubview(&l);
            return;
        };
        let icon = symbol_view(
            mtm,
            w.condition.symbol(w.is_day),
            20.0,
            &p.primary,
            rect(12.0, 12.0, 26.0, 22.0),
            w.condition.label(),
        );
        crate::views::multicolor(&icon);
        self.weather_card.addSubview(&icon);
        let temp = label(
            mtm,
            &temperature(w.temperature),
            &mono_digits(26.0, Weight::Semibold),
            &p.primary,
            rect(43.0, 8.0, 70.0, 30.0),
        );
        self.weather_card.addSubview(&temp);
        let detail = wrapping_label(
            mtm,
            &format!("{}\nH {} · L {}", w.condition.label(), temperature(w.high), temperature(w.low)),
            &font(11.0, Weight::Regular),
            &white(0.7),
            rect(wc - 112.0, 10.0, 100.0, 30.0),
            2,
        );
        detail.setAlignment(NSTextAlignment::Right);
        self.weather_card.addSubview(&detail);
        let n = 5usize.min(w.hourly.len() + 1);
        let cw = (wc - 16.0) / 5.0;
        for i in 0..n {
            let (hour, cond, t) = if i == 0 {
                ("Now".to_owned(), w.condition.symbol(w.is_day), w.temperature)
            } else {
                let hf = &w.hourly[i - 1];
                (hf.hour.clone(), hf.condition.symbol(true), hf.temperature)
            };
            let x = 8.0 + i as f64 * cw;
            let hl = label(mtm, &hour, &font(10.5, Weight::Regular), &white(0.65), rect(x, 48.0, cw, 13.0));
            hl.setAlignment(NSTextAlignment::Center);
            let ic = symbol_view(mtm, cond, 11.0, &p.primary, rect(x + (cw - 16.0) / 2.0, 63.0, 16.0, 14.0), "");
            crate::views::multicolor(&ic);
            let tl =
                label(mtm, &temperature(t), &mono_digits(10.5, Weight::Semibold), &p.primary, rect(x, 80.0, cw, 14.0));
            tl.setAlignment(NSTextAlignment::Center);
            self.weather_card.addSubview(&hl);
            self.weather_card.addSubview(&ic);
            self.weather_card.addSubview(&tl);
        }
        set_accessibility_label(
            &self.weather_card,
            &format!(
                "{}, {}, {}, high {} low {}",
                w.place,
                temperature(w.temperature),
                w.condition.label(),
                temperature(w.high),
                temperature(w.low)
            ),
        );
    }

    fn render_devices(&self, snap: &Snapshot) {
        let p = &self.palette;
        let mtm = self.mtm;
        remove_all_subviews(&self.devices);
        let w = self.status_width;
        if snap.devices.is_empty() {
            let l = label(
                mtm,
                "No accessories reporting battery",
                &font(11.5, Weight::Regular),
                &p.tertiary,
                rect(0.0, 3.0, w, 15.0),
            );
            self.devices.addSubview(&l);
            return;
        }
        let f = font(12.0, Weight::Regular);
        for (i, d) in snap.devices.iter().take(2).enumerate() {
            let y = i as f64 * 26.0;
            let icon = symbol_view(mtm, d.kind.symbol(), 12.0, &p.primary, rect(0.0, y + 4.0, 16.0, 15.0), &d.name);
            self.devices.addSubview(&icon);
            let mut parts: Vec<(String, Retained<NSColor>)> = Vec::new();
            for (j, (text, low)) in device_parts(&d.levels).into_iter().enumerate() {
                if j > 0 {
                    parts.push((" · ".into(), p.secondary.clone()));
                }
                parts.push((text, if low { p.warning.clone() } else { p.secondary.clone() }));
            }
            let levels = attributed_parts(&runs(&parts, &f));
            let lw = (crate::ui::text_width(&levels) + 8.0).min(w * 0.6);
            let lv = rich_label(mtm, &levels, rect(w - lw, y + 3.0, lw, 16.0));
            lv.setAlignment(NSTextAlignment::Right);
            set_accessibility_label(&lv, &format!("{} battery {}", d.name, super::device_levels(d)));
            self.devices.addSubview(&lv);
            let name =
                label(mtm, &d.name, &font(12.0, Weight::Medium), &p.primary, rect(23.0, y + 3.0, w - lw - 30.0, 16.0));
            self.devices.addSubview(&name);
        }
    }

    fn render_notifications(&self, snap: &Snapshot) {
        let p = &self.palette;
        remove_all_subviews(&self.notif_doc);
        let mut parts = vec![("Notifications".to_owned(), p.primary.clone())];
        if !snap.notifications.is_empty() {
            parts.push((format!("  {}", snap.notifications.len()), p.tertiary.clone()));
        }
        if snap.paused {
            parts.push(("  · paused".into(), p.warning.clone()));
        }
        let f = font(13.0, Weight::Semibold);
        self.notif_header.setAttributedStringValue(&attributed_parts(&runs(&parts, &f)));
        let visible_h = self.notif_scroll.frame().size.height;
        if snap.notifications.is_empty() {
            self.notif_doc.setFrame(rect(0.0, 0.0, self.notif_width, visible_h));
            let icon = symbol_view(
                self.mtm,
                "checkmark.circle",
                26.0,
                &p.tertiary,
                rect((self.notif_width - 32.0) / 2.0, visible_h / 2.0 - 40.0, 32.0, 32.0),
                "No notifications",
            );
            let l = label(
                self.mtm,
                "You're all caught up",
                &font(12.5, Weight::Medium),
                &p.tertiary,
                rect(0.0, visible_h / 2.0, self.notif_width, 17.0),
            );
            l.setAlignment(NSTextAlignment::Center);
            self.notif_doc.addSubview(&icon);
            self.notif_doc.addSubview(&l);
            return;
        }
        let mut y = 0.0;
        let mut views = Vec::new();
        for n in snap.notifications.iter().take(MAX_ROWS) {
            let rh = if n.body.trim().is_empty() { 50.0 } else { 64.0 };
            views.push(self.notification_row(n, snap.now_ms, rect(0.0, y, self.notif_width, rh)));
            y += rh + ROW_GAP;
        }
        self.notif_doc.setFrame(rect(0.0, 0.0, self.notif_width, y.max(visible_h)));
        for v in views {
            self.notif_doc.addSubview(&v);
        }
    }

    fn notification_row(&self, n: &Notification, now_ms: i64, frame: NSRect) -> Retained<FlippedView> {
        let p = &self.palette;
        let mtm = self.mtm;
        let w = frame.size.width;
        let urgent =
            n.priority >= Priority::High || matches!(n.kind, NotificationKind::NeedsInput | NotificationKind::Error);
        let row = FlippedView::new(mtm, frame);
        if urgent {
            make_card(&row, &rgba(1.0, 0.62, 0.04, 0.13), 14.0);
            if let Some(l) = row.layer() {
                l.setBorderWidth(0.5);
                l.setBorderColor(Some(&rgba(1.0, 0.62, 0.04, 0.35).CGColor()));
            }
        } else {
            make_card(&row, &white(0.07), 14.0);
        }
        let icon = app_icon_view(mtm, rect(12.0, 10.0, 26.0, 26.0));
        set_app_icon(&icon, &n.app);
        row.addSubview(&icon);

        let kind = match n.kind {
            NotificationKind::NeedsInput => Some("Needs input"),
            NotificationKind::TaskFinished => Some("Task finished"),
            NotificationKind::Error => Some("Error"),
            _ => None,
        };
        let mut head = n.app.display_name.clone();
        if let Some(k) = kind {
            head.push_str(" · ");
            head.push_str(k);
        }
        if n.group_count > 1 {
            head.push_str(&format!(" · {}", n.group_count));
        }
        let head_color = if urgent { rgba(1.0, 0.70, 0.25, 1.0) } else { p.secondary.clone() };
        let tx = 48.0;
        let time = label(
            mtm,
            &relative(n.timestamp_ms, now_ms),
            &font(11.0, Weight::Regular),
            &p.secondary,
            rect(w - 62.0, 9.0, 50.0, 14.0),
        );
        time.setAlignment(NSTextAlignment::Right);
        row.addSubview(&time);
        row.addSubview(&label(
            mtm,
            &head,
            &font(11.0, Weight::Semibold),
            &head_color,
            rect(tx, 9.0, w - tx - 66.0, 14.0),
        ));
        let mut title = n.title.clone();
        if let Some(sub) = &n.subtitle {
            title.push_str(" — ");
            title.push_str(sub);
        }
        row.addSubview(&label(
            mtm,
            &title,
            &font(13.0, Weight::Semibold),
            &p.primary,
            rect(tx, 24.0, w - tx - 12.0, 17.0),
        ));
        if !n.body.trim().is_empty() {
            row.addSubview(&label(
                mtm,
                &n.body.replace('\n', " "),
                &font(11.5, Weight::Regular),
                &p.secondary,
                rect(tx, 41.0, w - tx - 12.0, 15.0),
            ));
        }
        if !n.read {
            row.addSubview(&shape_view(mtm, rect(4.0, frame.size.height / 2.0 - 2.5, 5.0, 5.0), &p.accent, 2.5));
        }

        // Click opens; right-click offers Open, Copy and Dismiss.
        let id = n.id;
        let (url, bundle) = (n.url.clone(), n.app.bundle_id.clone());
        let open_fn = move || {
            let opened = url.as_deref().is_some_and(crate::sharing::open_url);
            if !opened {
                if let Some(b) = bundle.as_deref() {
                    crate::sharing::open_app(b);
                }
            }
        };
        let open_fn2 = open_fn.clone();
        let open = self.row_targets.add(mtm, move |_| open_fn());
        let overlay = click_overlay(mtm, &open, rect(0.0, 0.0, w, frame.size.height), &format!("Open {}", n.title));
        let text = n.plain_text();
        let eng = self.engine.clone();
        let entries: Vec<(&str, Action)> = vec![
            ("Open", Box::new(open_fn2)),
            ("Copy", Box::new(move || crate::sharing::copy_text(&text))),
            ("Dismiss", Box::new(move || eng.send(Command::DismissNotification(id)))),
        ];
        // SAFETY: the menu's targets are kept alive by `row_targets`.
        unsafe { overlay.setMenu(Some(&self.menu(entries))) };
        row.addSubview(&overlay);
        set_accessibility_label(&row, &format!("{}, {}", n.plain_text(), relative_spoken(n.timestamp_ms, now_ms)));
        row
    }

    fn render_tray(&self, snap: &Snapshot) {
        let p = &self.palette;
        let mtm = self.mtm;
        remove_all_subviews(&self.tray_doc);
        *self.tray_paths.borrow_mut() = snap.tray.iter().map(|t| t.path.clone()).collect();
        let f = font(13.0, Weight::Semibold);
        let count = if snap.tray.is_empty() { String::new() } else { format!("  {}", snap.tray.len()) };
        self.tray_header
            .setAttributedStringValue(&attributed_parts(&[("Tray", &f, &p.primary), (&count, &f, &p.tertiary)]));
        self.tray_empty.setHidden(!snap.tray.is_empty());
        self.tray_scroll.setHidden(snap.tray.is_empty());
        let rows = snap.tray.len().div_ceil(2);
        let visible_h = self.tray_scroll.frame().size.height;
        self.tray_doc.setFrame(rect(0.0, 0.0, self.tray_width, (rows as f64 * (TILE_H + 8.0) + 16.0).max(visible_h)));
        let cw = (self.tray_width - 24.0 - 8.0) / 2.0;
        for (i, item) in snap.tray.iter().enumerate() {
            let (c, r) = (i % 2, i / 2);
            let tile = TrayItemView::create(
                mtm,
                rect(12.0 + c as f64 * (cw + 8.0), 12.0 + r as f64 * (TILE_H + 8.0), cw, TILE_H),
                item.path.clone(),
            );
            let icon = NSImageView::imageViewWithImage(&crate::icons::file_icon(&item.path), mtm);
            icon.setImageScaling(objc2_app_kit::NSImageScaling::ScaleProportionallyUpOrDown);
            icon.setFrame(rect((cw - 54.0) / 2.0, 4.0, 54.0, 54.0));
            tile.addSubview(&icon);
            let name = label(mtm, &item.name, &font(11.0, Weight::Regular), &p.primary, rect(0.0, 64.0, cw, 14.0));
            name.setAlignment(NSTextAlignment::Center);
            name.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
            tile.addSubview(&name);
            let size = item
                .bytes
                .map(format_bytes)
                .unwrap_or_else(|| if item.is_dir { "Folder".into() } else { String::new() });
            tile.setToolTip(Some(&crate::ui::ns(&format!(
                "{}\n{}\nDrag out · double-click to open · right-click for actions",
                item.path.display(),
                size
            ))));
            set_accessibility_label(&tile, &format!("{} in tray. Drag to move it out.", item.name));
            let (p1, p2, p3, p4, id) =
                (item.path.clone(), item.path.clone(), item.path.clone(), item.path.clone(), item.id);
            let eng = self.engine.clone();
            let entries: Vec<(&str, Action)> = vec![
                (
                    "Open",
                    Box::new(move || {
                        crate::sharing::open_path(&p1);
                    }),
                ),
                ("Reveal in Finder", Box::new(move || crate::sharing::reveal(std::slice::from_ref(&p2)))),
                (
                    "AirDrop…",
                    Box::new(move || {
                        crate::sharing::airdrop(std::slice::from_ref(&p3));
                    }),
                ),
                ("Copy Path", Box::new(move || crate::sharing::copy_paths(std::slice::from_ref(&p4)))),
                ("Remove from Tray", Box::new(move || eng.send(Command::TrayRemove(id)))),
            ];
            // SAFETY: the menu's targets are kept alive by `row_targets`.
            unsafe { tile.setMenu(Some(&self.menu(entries))) };
            self.tray_doc.addSubview(&tile);
        }
    }

    fn menu(&self, entries: Vec<(&str, Action)>) -> Retained<NSMenu> {
        let mtm = self.mtm;
        let menu = NSMenu::new(mtm);
        for (title, f) in entries {
            let t = self.row_targets.add(mtm, move |_| f());
            // SAFETY: valid selector; target kept alive by row_targets.
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &crate::ui::ns(title),
                    Some(crate::ui::ActionTarget::action()),
                    &crate::ui::ns(""),
                )
            };
            // SAFETY: target kept alive by row_targets.
            unsafe { item.setTarget(Some(t.as_any())) };
            menu.addItem(&item);
        }
        menu
    }

    /// Read live control values; called each time the panel expands.
    pub fn refresh_controls(&self, display_id: u32) {
        let cfg = self.config.borrow().clone();
        let b = if cfg.controls.brightness { brightness::get() } else { None };
        self.brightness_row.setHidden(b.is_none());
        if let Some(v) = b {
            self.brightness.set_value(v as f64);
        }
        let v = if cfg.controls.volume { volume::get() } else { None };
        self.volume_row.setHidden(v.is_none());
        if let Some(v) = v {
            let muted = volume::muted().unwrap_or(false);
            self.volume.set_value(if muted { 0.0 } else { v as f64 });
        }

        self.resolution.setHidden(!cfg.controls.resolution);
        if cfg.controls.resolution {
            self.display_id.set(display_id);
            let name = crate::screen::display_name(self.mtm, display_id);
            let info = display_modes::display_info(display_id, name);
            let current = info.modes.iter().find(|m| Some(m.id) == info.current_mode_id);
            let title = current.map(|m| format!("{} × {}", m.width, m.height)).unwrap_or_else(|| "Resolution".into());
            style_pill(
                &self.resolution,
                &title,
                Some("display"),
                &font(12.0, Weight::Medium),
                &self.palette.primary,
                &white(0.08),
                8.0,
            );
            self.resolution.setHidden(info.modes.is_empty());
            *self.display.borrow_mut() = Some(info);
        }
    }

    /// The resolution pill's menu: every usable mode, current one checked.
    pub fn resolution_menu(&self, anchor: &NSButton) {
        let Some(info) = self.display.borrow().clone() else { return };
        let mtm = self.mtm;
        let menu = NSMenu::new(mtm);
        let targets = Targets::default();
        for (i, m) in info.modes.iter().enumerate() {
            let t = targets.add(mtm, move |_| {
                // Deferred: applying shows a modal keep/revert confirmation.
                dispatch2::DispatchQueue::main().exec_async(move || {
                    with_controller(|c| c.choose_display_mode(i as isize));
                });
            });
            // SAFETY: valid selector; the target outlives the modal menu below.
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &crate::ui::ns(&m.label()),
                    Some(crate::ui::ActionTarget::action()),
                    &crate::ui::ns(""),
                )
            };
            // SAFETY: as above.
            unsafe { item.setTarget(Some(t.as_any())) };
            if Some(m.id) == info.current_mode_id {
                item.setState(objc2_app_kit::NSControlStateValueOn);
            }
            menu.addItem(&item);
        }
        let size = anchor.frame().size;
        menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(0.0, size.height + 4.0), Some(anchor));
        drop(targets);
    }

    /// Display + mode for a menu index.
    pub fn display_mode_at(&self, idx: isize) -> Option<(u32, i32)> {
        let d = self.display.borrow();
        let info = d.as_ref()?;
        let m = info.modes.get(usize::try_from(idx).ok()?)?;
        Some((self.display_id.get(), m.id))
    }
}
