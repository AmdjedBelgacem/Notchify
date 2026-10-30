//! Expanded (pinned) panel: notifications · file tray · status & controls.
//!
//! ```text
//! ┌──────── 13° Paris ──────[ notch ]────── 82% ⚡ ────────┐
//! │ Notifications      🗑 │ Tray (3)        │ ☁ 13°  Cloudy  │
//! │ ┌───────────────────┐ │ ┌─────────────┐ │ H 14° · L 8°   │
//! │ │ ✦ Claude · 2m  ↗⧉✕│ │ │ 📄  📁  📄 │ │ 15 16 17 18 19 │
//! │ │ Needs your input  │ │ │             │ │ AirPods  80%   │
//! │ └───────────────────┘ │ └─────────────┘ │ ☀ ━━━━━○──     │
//! │ …                     │ ⇪  📂  ⧉  🗑   │ 🔊 ━━━○────    │
//! │                       │                 │ [1512 × 982 ▾] │
//! │                       │                 │ ☾  🔔  ⚙       │
//! └──────────────────────────────────────────────────────────┘
//! ```

use super::{app_icon_view, battery_text, device_levels, set_app_icon, temperature};
use crate::app::with_controller;
use crate::controls::{brightness, display_modes, volume};
use crate::panel::TrayItemView;
use crate::ui::{
    FlippedView, Palette, Targets, Weight, font, icon_button, label, make_card, mono_digits, rect, remove_all_subviews,
    set_accessibility_label, set_symbol, set_text, symbol_view, wrapping_label,
};
use notchforge_core::config::Config;
use notchforge_core::engine::{Command, EngineHandle, Snapshot};
use notchforge_core::geometry::{LayoutKind, NotchLayout};
use notchforge_core::model::{DisplayInfo, Notification, NotificationKind, Priority};
use notchforge_core::time::{relative, relative_spoken};
use notchforge_core::tray::format_bytes;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSControlSize, NSImageView, NSPopUpButton, NSScrollView, NSScrollerStyle, NSSlider, NSTextAlignment, NSTextField,
    NSView,
};
use objc2_foundation::NSRect;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::Arc;

const PAD: f64 = 16.0;
const GAP: f64 = 12.0;
const ROW_H: f64 = 66.0;
const ROW_GAP: f64 = 6.0;
const MAX_ROWS: usize = 40;
const TILE: f64 = 72.0;

type TrayAction = Box<dyn Fn(Vec<PathBuf>, &EngineHandle)>;
type Action = Box<dyn Fn()>;

pub struct ExpandedView {
    pub root: Retained<FlippedView>,
    mtm: MainThreadMarker,
    palette: Palette,
    engine: EngineHandle,
    config: RefCell<Arc<Config>>,
    /// Keeps the static controls' action targets alive.
    _targets: Targets,
    row_targets: Targets,

    top_left: Retained<NSTextField>,
    top_right: Retained<NSTextField>,

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

    weather_icon: Retained<NSImageView>,
    weather_temp: Retained<NSTextField>,
    weather_desc: Retained<NSTextField>,
    hourly: Retained<FlippedView>,
    devices: Retained<FlippedView>,
    status_width: f64,

    brightness_row: Retained<FlippedView>,
    brightness: Retained<NSSlider>,
    volume_row: Retained<FlippedView>,
    volume: Retained<NSSlider>,
    resolution: Retained<NSPopUpButton>,
    display: RefCell<Option<DisplayInfo>>,
    display_id: Cell<u32>,
    pause_button: RefCell<Option<Retained<objc2_app_kit::NSButton>>>,

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

impl ExpandedView {
    pub fn new(mtm: MainThreadMarker, layout: &NotchLayout, palette: Palette, config: Arc<Config>, engine: EngineHandle) -> Self {
        let p = &palette;
        let s = layout.shape(LayoutKind::Expanded);
        let (w, h) = (s.width, s.height);
        let nh = layout.notch.rect.height;
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        let targets = Targets::default();

        // ── top row beside the notch ──
        let side = (w - layout.notch.rect.width) / 2.0 - PAD - 12.0;
        let ty = (nh - 16.0) / 2.0;
        let top_left = label(mtm, "", &mono_digits(12.5, Weight::Semibold), &p.primary, rect(PAD + 6.0, ty, side, 16.0));
        let top_right = label(mtm, "", &mono_digits(12.5, Weight::Semibold), &p.primary, rect(w - PAD - 6.0 - side, ty, side, 16.0));
        top_right.setAlignment(NSTextAlignment::Right);
        root.addSubview(&top_left);
        root.addSubview(&top_right);

        let body_y = nh + 8.0;
        let body_h = h - body_y - 14.0;
        let c1w = (w * 0.42).floor();
        let c2w = (w * 0.25).floor();
        let c1x = PAD;
        let c2x = c1x + c1w + GAP;
        let c3x = c2x + c2w + GAP;
        let c3w = w - PAD - c3x;

        // ── notifications column ──
        let notif_header = label(mtm, "Notifications", &font(12.0, Weight::Semibold), &p.secondary, rect(c1x + 2.0, body_y, c1w - 60.0, 16.0));
        root.addSubview(&notif_header);
        let eng = engine.clone();
        let t = targets.add(mtm, move |_| eng.send(Command::ClearNotifications));
        root.addSubview(&icon_button(mtm, "trash", "Clear all notifications", &p.secondary, &t, rect(c1x + c1w - 22.0, body_y - 1.0, 20.0, 18.0)));
        let notif_doc = FlippedView::new(mtm, rect(0.0, 0.0, c1w, body_h - 22.0));
        let notif_scroll = scroll_view(mtm, rect(c1x, body_y + 22.0, c1w, body_h - 22.0), &notif_doc);
        root.addSubview(&notif_scroll);

        // ── tray column ──
        let tray_header = label(mtm, "Tray", &font(12.0, Weight::Semibold), &p.secondary, rect(c2x + 2.0, body_y, c2w - 4.0, 16.0));
        root.addSubview(&tray_header);
        let box_h = body_h - 22.0 - 30.0;
        let tray_box = FlippedView::new(mtm, rect(c2x, body_y + 22.0, c2w, box_h));
        make_card(&tray_box, &p.card, 14.0);
        let tray_doc = FlippedView::new(mtm, rect(0.0, 0.0, c2w, box_h));
        let tray_scroll = scroll_view(mtm, rect(0.0, 0.0, c2w, box_h), &tray_doc);
        tray_box.addSubview(&tray_scroll);
        let tray_empty = FlippedView::new(mtm, rect(0.0, 0.0, c2w, box_h));
        tray_empty.addSubview(&symbol_view(mtm, "tray.and.arrow.down", 26.0, &p.tertiary, rect((c2w - 34.0) / 2.0, box_h / 2.0 - 30.0, 34.0, 30.0), "Drop files"));
        let hint = wrapping_label(mtm, "Drop files here to hold them", &font(11.0, Weight::Regular), &p.tertiary, rect(10.0, box_h / 2.0 + 6.0, c2w - 20.0, 30.0), 2);
        hint.setAlignment(NSTextAlignment::Center);
        tray_empty.addSubview(&hint);
        tray_box.addSubview(&tray_empty);
        root.addSubview(&tray_box);

        let by = body_y + 22.0 + box_h + 6.0;
        let bw = 26.0;
        let actions: [(&str, &str, TrayAction); 4] = [
            ("dot.radiowaves.left.and.right", "AirDrop tray files", Box::new(|paths, _| {
                crate::sharing::airdrop(&paths);
            })),
            ("folder", "Reveal tray files in Finder", Box::new(|paths, _| crate::sharing::reveal(&paths))),
            ("doc.on.clipboard", "Copy file paths", Box::new(|paths, _| crate::sharing::copy_paths(&paths))),
            ("trash", "Clear tray", Box::new(|_, eng| eng.send(Command::TrayClear))),
        ];
        let step = c2w / actions.len() as f64;
        for (i, (sym, tip, action)) in actions.into_iter().enumerate() {
            let eng = engine.clone();
            let t = targets.add(mtm, move |_| {
                let paths = with_controller(|c| c.tray_paths()).unwrap_or_default();
                action(paths, &eng);
            });
            root.addSubview(&icon_button(mtm, sym, tip, &p.secondary, &t, rect(c2x + i as f64 * step + (step - bw) / 2.0, by, bw, 22.0)));
        }

        // ── status & controls column ──
        let weather_card = FlippedView::new(mtm, rect(c3x, body_y, c3w, 104.0));
        make_card(&weather_card, &p.card, 14.0);
        let weather_icon = symbol_view(mtm, "cloud.sun.fill", 18.0, &p.primary, rect(10.0, 9.0, 22.0, 22.0), "Weather");
        let weather_temp = label(mtm, "--°", &mono_digits(19.0, Weight::Semibold), &p.primary, rect(38.0, 7.0, c3w - 46.0, 24.0));
        let weather_desc = label(mtm, "", &font(10.5, Weight::Regular), &p.secondary, rect(10.0, 34.0, c3w - 18.0, 14.0));
        let hourly = FlippedView::new(mtm, rect(6.0, 52.0, c3w - 12.0, 46.0));
        for v in [&*weather_icon as &NSView, &weather_temp, &weather_desc, &hourly] {
            weather_card.addSubview(v);
        }
        root.addSubview(&weather_card);

        let devices = FlippedView::new(mtm, rect(c3x, body_y + 110.0, c3w, 50.0));
        root.addSubview(&devices);

        let mut y = body_y + 164.0;
        let slider_row = |sym: &str, tip: &str, action: Box<dyn Fn(f32)>, y: f64| {
            let row = FlippedView::new(mtm, rect(c3x, y, c3w, 22.0));
            row.addSubview(&symbol_view(mtm, sym, 12.0, &p.secondary, rect(2.0, 3.0, 16.0, 16.0), tip));
            let t = targets.add(mtm, move |sender: Option<&AnyObject>| {
                if let Some(s) = sender.and_then(|s| s.downcast_ref::<NSSlider>()) {
                    action(s.floatValue());
                }
            });
            // SAFETY: target kept alive by `targets`.
            let slider = unsafe {
                NSSlider::sliderWithValue_minValue_maxValue_target_action(0.5, 0.0, 1.0, Some(t.as_any()), Some(crate::ui::ActionTarget::action()), mtm)
            };
            slider.setControlSize(NSControlSize::Small);
            slider.setContinuous(true);
            slider.setFrame(rect(24.0, 2.0, c3w - 26.0, 18.0));
            set_accessibility_label(&slider, tip);
            row.addSubview(&slider);
            root.addSubview(&row);
            (row, slider)
        };
        let (brightness_row, brightness_slider) = slider_row(
            "sun.max.fill",
            "Display brightness",
            Box::new(|v| {
                brightness::set(v);
            }),
            y,
        );
        y += 26.0;
        let (volume_row, volume_slider) = slider_row(
            "speaker.wave.2.fill",
            "Output volume",
            Box::new(|v| {
                volume::set(v);
            }),
            y,
        );
        y += 28.0;

        let resolution = NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), rect(c3x - 2.0, y, c3w + 4.0, 24.0), false);
        resolution.setControlSize(NSControlSize::Small);
        resolution.setFont(Some(&font(11.0, Weight::Regular)));
        set_accessibility_label(&resolution, "Display resolution");
        let t = targets.add(mtm, move |sender: Option<&AnyObject>| {
            if let Some(popup) = sender.and_then(|s| s.downcast_ref::<NSPopUpButton>()) {
                let idx = popup.indexOfSelectedItem();
                with_controller(|c| c.choose_display_mode(idx));
            }
        });
        // SAFETY: target kept alive by `targets`.
        unsafe {
            resolution.setTarget(Some(t.as_any()));
            resolution.setAction(Some(crate::ui::ActionTarget::action()));
        }
        root.addSubview(&resolution);
        y += 30.0;

        let buttons: [(&str, &str, Action); 4] = [
            ("moon.fill", "Toggle Focus", Box::new(|| {
                with_controller(|c| c.toggle_focus());
            })),
            ("bell.slash", "Pause notifications", Box::new(|| {
                with_controller(|c| c.toggle_pause());
            })),
            ("gearshape", "Settings", Box::new(crate::app::show_settings)),
            ("chevron.up", "Collapse", Box::new(|| {
                with_controller(|c| c.collapse());
            })),
        ];
        let step = c3w / buttons.len() as f64;
        let pause_button = RefCell::new(None);
        for (i, (sym, tip, action)) in buttons.into_iter().enumerate() {
            let t = targets.add(mtm, move |_| action());
            let b = icon_button(mtm, sym, tip, &p.secondary, &t, rect(c3x + i as f64 * step + (step - 26.0) / 2.0, y, 26.0, 22.0));
            if i == 1 {
                *pause_button.borrow_mut() = Some(b.clone());
            }
            root.addSubview(&b);
        }

        Self {
            root,
            mtm,
            palette,
            engine,
            config: RefCell::new(config),
            _targets: targets,
            row_targets: Targets::default(),
            top_left,
            top_right,
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
            weather_icon,
            weather_temp,
            weather_desc,
            hourly,
            devices,
            status_width: c3w,
            brightness_row,
            brightness: brightness_slider,
            volume_row,
            volume: volume_slider,
            resolution,
            display: RefCell::new(None),
            display_id: Cell::new(layout.screen.id),
            pause_button,
            last_rendered: Cell::new(0),
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
                layer.setBorderWidth(0.0);
            }
        }
    }

    /// Re-render dynamic content. Skips work if this snapshot was rendered.
    pub fn update(&self, snap: &Snapshot) {
        if self.last_rendered.replace(snap.generation) == snap.generation {
            return;
        }
        let p = &self.palette;
        self.row_targets.clear();

        // Top row.
        let left = snap.weather.as_ref().map(|w| format!("{}  {}", temperature(w.temperature), w.place)).unwrap_or_default();
        set_text(&self.top_left, &left);
        let right = snap
            .battery
            .as_ref()
            .filter(|b| b.present)
            .map(|b| format!("{}{}", battery_text(b).0, if b.charging { " ⚡︎" } else { "" }))
            .unwrap_or_default();
        set_text(&self.top_right, &right);

        self.render_notifications(snap);
        self.render_tray(snap);

        // Weather card.
        match &snap.weather {
            Some(w) => {
                set_symbol(&self.weather_icon, w.condition.symbol(w.is_day), 18.0, w.condition.label());
                set_text(&self.weather_temp, &format!("{}  {}", temperature(w.temperature), w.condition.label()));
                set_text(&self.weather_desc, &format!("{} · H {} L {} · feels {}", w.place, temperature(w.high), temperature(w.low), temperature(w.feels_like)));
                remove_all_subviews(&self.hourly);
                let n = 5usize.min(w.hourly.len());
                let cw = (self.status_width - 12.0) / 5.0;
                for (i, hf) in w.hourly.iter().take(n).enumerate() {
                    let x = i as f64 * cw;
                    let hl = label(self.mtm, &hf.hour, &font(9.5, Weight::Regular), &p.tertiary, rect(x, 0.0, cw, 12.0));
                    hl.setAlignment(NSTextAlignment::Center);
                    let icon = symbol_view(self.mtm, hf.condition.symbol(true), 11.0, &p.secondary, rect(x + (cw - 16.0) / 2.0, 13.0, 16.0, 16.0), hf.condition.label());
                    let tl = label(self.mtm, &temperature(hf.temperature), &mono_digits(10.0, Weight::Medium), &p.primary, rect(x, 30.0, cw, 13.0));
                    tl.setAlignment(NSTextAlignment::Center);
                    self.hourly.addSubview(&hl);
                    self.hourly.addSubview(&icon);
                    self.hourly.addSubview(&tl);
                }
            }
            None => {
                set_text(&self.weather_temp, "--°");
                set_text(&self.weather_desc, "Weather unavailable");
                remove_all_subviews(&self.hourly);
            }
        }

        // Devices.
        remove_all_subviews(&self.devices);
        for (i, d) in snap.devices.iter().take(3).enumerate() {
            let y = i as f64 * 16.0;
            let icon = symbol_view(self.mtm, d.kind.symbol(), 10.0, &p.secondary, rect(2.0, y + 1.0, 14.0, 13.0), &d.name);
            let low = d.min_level().is_some_and(|l| l <= 20);
            let text = format!("{}  {}", d.name, device_levels(d));
            let l = label(self.mtm, &text, &font(10.5, Weight::Regular), if low { &p.warning } else { &p.secondary }, rect(20.0, y, self.status_width - 22.0, 14.0));
            set_accessibility_label(&l, &format!("{} battery {}", d.name, device_levels(d)));
            self.devices.addSubview(&icon);
            self.devices.addSubview(&l);
        }
        if snap.devices.is_empty() {
            let l = label(self.mtm, "No accessories reporting battery", &font(10.5, Weight::Regular), &p.tertiary, rect(2.0, 0.0, self.status_width - 4.0, 14.0));
            self.devices.addSubview(&l);
        }

        if let Some(b) = self.pause_button.borrow().as_ref() {
            b.setContentTintColor(Some(if snap.paused { &p.warning } else { &p.secondary }));
            set_accessibility_label(b, if snap.paused { "Resume notifications" } else { "Pause notifications" });
        }
    }

    fn render_notifications(&self, snap: &Snapshot) {
        let p = &self.palette;
        remove_all_subviews(&self.notif_doc);
        let mut header = "Notifications".to_owned();
        if !snap.notifications.is_empty() {
            header.push_str(&format!("  {}", snap.notifications.len()));
        }
        if snap.paused {
            header.push_str("  · paused");
        }
        set_text(&self.notif_header, &header);
        let visible_h = self.notif_scroll.frame().size.height;
        if snap.notifications.is_empty() {
            self.notif_doc.setFrame(rect(0.0, 0.0, self.notif_width, visible_h));
            let icon = symbol_view(self.mtm, "checkmark.circle", 24.0, &p.tertiary, rect((self.notif_width - 30.0) / 2.0, visible_h / 2.0 - 34.0, 30.0, 30.0), "No notifications");
            let l = label(self.mtm, "You're all caught up", &font(12.0, Weight::Medium), &p.tertiary, rect(0.0, visible_h / 2.0 + 2.0, self.notif_width, 16.0));
            l.setAlignment(NSTextAlignment::Center);
            self.notif_doc.addSubview(&icon);
            self.notif_doc.addSubview(&l);
            return;
        }
        let rows: Vec<&Notification> = snap.notifications.iter().take(MAX_ROWS).collect();
        let doc_h = (rows.len() as f64 * (ROW_H + ROW_GAP)).max(visible_h);
        self.notif_doc.setFrame(rect(0.0, 0.0, self.notif_width, doc_h));
        for (i, n) in rows.into_iter().enumerate() {
            let row = self.notification_row(n, snap.now_ms, rect(0.0, i as f64 * (ROW_H + ROW_GAP), self.notif_width - 2.0, ROW_H));
            self.notif_doc.addSubview(&row);
        }
    }

    fn notification_row(&self, n: &Notification, now_ms: i64, frame: NSRect) -> Retained<FlippedView> {
        let p = &self.palette;
        let mtm = self.mtm;
        let w = frame.size.width;
        let row = FlippedView::new(mtm, frame);
        make_card(&row, &p.card, 12.0);
        if n.priority >= Priority::High {
            if let Some(layer) = row.layer() {
                layer.setBorderWidth(1.0);
                layer.setBorderColor(Some(&(if n.kind == NotificationKind::Error { &p.warning } else { &p.accent }).CGColor()));
            }
        }
        let icon = app_icon_view(mtm, rect(10.0, 10.0, 22.0, 22.0));
        set_app_icon(&icon, &n.app);
        row.addSubview(&icon);

        let mut header = format!("{} · {}", n.app.display_name, relative(n.timestamp_ms, now_ms));
        if n.group_count > 1 {
            header.push_str(&format!(" · {} messages", n.group_count));
        }
        if !n.read {
            header.insert_str(0, "● ");
        }
        row.addSubview(&label(mtm, &header, &font(10.5, Weight::Medium), &p.tertiary, rect(40.0, 7.0, w - 40.0 - 74.0, 13.0)));
        let title_color = match n.kind {
            NotificationKind::Error => &p.warning,
            _ => &p.primary,
        };
        let mut title = n.title.clone();
        if let Some(sub) = &n.subtitle {
            title.push_str(" — ");
            title.push_str(sub);
        }
        row.addSubview(&label(mtm, &title, &font(12.5, Weight::Semibold), title_color, rect(40.0, 21.0, w - 50.0, 16.0)));
        row.addSubview(&wrapping_label(mtm, &n.body.replace('\n', " "), &font(11.0, Weight::Regular), &p.secondary, rect(40.0, 37.0, w - 50.0, 26.0), 2));

        // Quick actions: open, copy, dismiss.
        let id = n.id;
        let (url, bundle) = (n.url.clone(), n.app.bundle_id.clone());
        let open = self.row_targets.add(mtm, move |_| {
            let opened = url.as_deref().is_some_and(crate::sharing::open_url);
            if !opened {
                if let Some(b) = bundle.as_deref() {
                    crate::sharing::open_app(b);
                }
            }
        });
        let text = n.plain_text();
        let copy = self.row_targets.add(mtm, move |_| crate::sharing::copy_text(&text));
        let eng = self.engine.clone();
        let dismiss = self.row_targets.add(mtm, move |_| eng.send(Command::DismissNotification(id)));
        let bx = w - 70.0;
        if n.url.is_some() || n.app.bundle_id.is_some() {
            row.addSubview(&icon_button(mtm, "arrow.up.forward.app", "Open", &p.secondary, &open, rect(bx, 4.0, 20.0, 18.0)));
        }
        row.addSubview(&icon_button(mtm, "doc.on.doc", "Copy", &p.secondary, &copy, rect(bx + 22.0, 4.0, 20.0, 18.0)));
        row.addSubview(&icon_button(mtm, "xmark", "Dismiss", &p.secondary, &dismiss, rect(bx + 44.0, 4.0, 20.0, 18.0)));
        set_accessibility_label(&row, &format!("{}, {}", n.plain_text(), relative_spoken(n.timestamp_ms, now_ms)));
        row
    }

    fn render_tray(&self, snap: &Snapshot) {
        let p = &self.palette;
        let mtm = self.mtm;
        remove_all_subviews(&self.tray_doc);
        *self.tray_paths.borrow_mut() = snap.tray.iter().map(|t| t.path.clone()).collect();
        let header = if snap.tray.is_empty() { "Tray".to_owned() } else { format!("Tray  {}", snap.tray.len()) };
        set_text(&self.tray_header, &header);
        self.tray_empty.setHidden(!snap.tray.is_empty());
        self.tray_scroll.setHidden(snap.tray.is_empty());
        let cols = ((self.tray_width - 8.0) / TILE).floor().max(1.0) as usize;
        let rows = snap.tray.len().div_ceil(cols);
        let visible_h = self.tray_scroll.frame().size.height;
        self.tray_doc.setFrame(rect(0.0, 0.0, self.tray_width, (rows as f64 * (TILE + 4.0) + 8.0).max(visible_h)));
        let x0 = (self.tray_width - cols as f64 * TILE) / 2.0;
        for (i, item) in snap.tray.iter().enumerate() {
            let (c, r) = (i % cols, i / cols);
            let tile = TrayItemView::create(mtm, rect(x0 + c as f64 * TILE, 6.0 + r as f64 * (TILE + 4.0), TILE, TILE), item.path.clone());
            let icon = objc2_app_kit::NSImageView::imageViewWithImage(&crate::icons::file_icon(&item.path), mtm);
            icon.setFrame(rect((TILE - 36.0) / 2.0, 4.0, 36.0, 36.0));
            tile.addSubview(&icon);
            let name = wrapping_label(mtm, &item.name, &font(10.0, Weight::Regular), &p.primary, rect(2.0, 42.0, TILE - 4.0, 28.0), 2);
            name.setAlignment(NSTextAlignment::Center);
            tile.addSubview(&name);
            let size = item.bytes.map(format_bytes).unwrap_or_else(|| if item.is_dir { "Folder".into() } else { String::new() });
            tile.setToolTip(Some(&crate::ui::ns(&format!("{}\n{}\nDrag out · double-click to open · right-click for actions", item.path.display(), size))));
            set_accessibility_label(&tile, &format!("{} in tray. Drag to move it out.", item.name));
            // SAFETY: the menu's targets are kept alive by `row_targets`.
            unsafe { tile.setMenu(Some(&self.tray_item_menu(item.id, item.path.clone()))) };
            self.tray_doc.addSubview(&tile);
        }
    }

    fn tray_item_menu(&self, id: notchforge_core::model::TrayItemId, path: PathBuf) -> Retained<objc2_app_kit::NSMenu> {
        let mtm = self.mtm;
        let menu = objc2_app_kit::NSMenu::new(mtm);
        let entries: [(&str, Action); 5] = {
            let (p1, p2, p3, p4) = (path.clone(), path.clone(), path.clone(), path);
            let eng = self.engine.clone();
            [
                ("Open", Box::new(move || {
                    crate::sharing::open_path(&p1);
                })),
                ("Reveal in Finder", Box::new(move || crate::sharing::reveal(std::slice::from_ref(&p2)))),
                ("AirDrop…", Box::new(move || {
                    crate::sharing::airdrop(std::slice::from_ref(&p3));
                })),
                ("Copy Path", Box::new(move || crate::sharing::copy_paths(std::slice::from_ref(&p4)))),
                ("Remove from Tray", Box::new(move || eng.send(Command::TrayRemove(id)))),
            ]
        };
        for (title, f) in entries {
            let t = self.row_targets.add(mtm, move |_| f());
            // SAFETY: valid selector; target kept alive by row_targets.
            let item = unsafe {
                objc2_app_kit::NSMenuItem::initWithTitle_action_keyEquivalent(
                    objc2_app_kit::NSMenuItem::alloc(mtm),
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
            self.brightness.setDoubleValue(v as f64);
        }
        let v = if cfg.controls.volume { volume::get() } else { None };
        self.volume_row.setHidden(v.is_none());
        if let Some(v) = v {
            let muted = volume::muted().unwrap_or(false);
            self.volume.setDoubleValue(if muted { 0.0 } else { v as f64 });
        }

        self.resolution.setHidden(!cfg.controls.resolution);
        if cfg.controls.resolution {
            self.display_id.set(display_id);
            let name = crate::screen::display_name(self.mtm, display_id);
            let info = display_modes::display_info(display_id, name);
            self.resolution.removeAllItems();
            let mut selected = None;
            for (i, m) in info.modes.iter().enumerate() {
                self.resolution.addItemWithTitle(&crate::ui::ns(&m.label()));
                if Some(m.id) == info.current_mode_id {
                    selected = Some(i);
                }
            }
            if let Some(i) = selected {
                self.resolution.selectItemAtIndex(i as isize);
            }
            self.resolution.setHidden(info.modes.is_empty());
            *self.display.borrow_mut() = Some(info);
        }
    }

    /// Display + mode for a popup index.
    pub fn display_mode_at(&self, idx: isize) -> Option<(u32, i32)> {
        let d = self.display.borrow();
        let info = d.as_ref()?;
        let m = info.modes.get(usize::try_from(idx).ok()?)?;
        Some((self.display_id.get(), m.id))
    }
}
