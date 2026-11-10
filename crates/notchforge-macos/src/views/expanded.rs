//! Expanded (pinned) panel, built to the *Pinned · Overview, glass* board of
//! the NotchForge design canvas. Two pages share the top row: **Overview**
//! (this file) and **Now Playing** ([`super::media`]).
//!
//! ```text
//! ┌(▦|♪) Fri 31 Oct 21:14 ───[ notch ]──── (▮ Afterglow · Neon) 82% ▭ ┐
//! │ Notifications       (×) │ ╭ Paris        ⛅ ╮ │  (☾) (🔕) (⚙) (⌃)    │
//! │ ╭ ✳ Claude Code   now ╮ │ │ 10°             │ │ (☀▓▓▓▓▓▓▓▓▓░░░░░░░) │
//! │ │ Allow running tests?│ │ │ Partly cloudy   │ │ (🔈▓▓▓▓▓▓▓░░░░░░░░) │
//! │ │ (  Deny  ) ( Allow )│ │ │ Now 22 23 00 01 │ │ (▭ 1512 × 982     ⇅) │
//! │ ╰─────────────────────╯ │ ╰─────────────────╯ │ ╭ Batteries         ╮ │
//! │ ╭ ▸ Cursor         6m ╮ │ ╭ Tray            ╮ │ │  ◯    ◯    ◯      │ │
//! │ ╰─────────────────────╯ │ │ 📄  🖼  📁  ⦿▭⧉🗑│ │ │ 80%  18%  64%     │ │
//! └───────────────────────────────────────────────────────────────────┘
//! ```
//!
//! Every column ends the same distance from the panel's edge, so nothing is
//! left empty. Notification rows open on click; Open, Copy and Dismiss are on
//! the right-click menu, as in Notification Center.

use super::media::MediaPage;
use super::usage::UsagePage;
use super::{app_icon_view, set_app_icon, temperature};
use crate::app::with_controller;
use crate::controls::{brightness, display_modes, mic, volume};
use crate::panel::TrayItemView;
use crate::ui::{
    FlippedView, Palette, Targets, ThinSlider, Weight, attributed, attributed_parts, click_overlay, font, label,
    make_card, mono_digits, pill_button, rect, remove_all_subviews, rgba, set_accessibility_label, set_button_symbol,
    shape_view, style_pill, symbol_button, symbol_view, wrapping_label,
};
use notchforge_core::config::Config;
use notchforge_core::engine::{Command, EngineHandle, Snapshot};
use notchforge_core::geometry::{LayoutKind, NotchLayout};
use notchforge_core::model::{
    AuthTarget, BatteryComponent, BatteryStatus, CalendarState, DisplayInfo, Notification, NotificationKind, Priority,
};
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

const PAD: f64 = 14.0;
const GAP: f64 = 10.0;
const ROW_GAP: f64 = 10.0;
const MAX_ROWS: usize = 40;
const TILE_H: f64 = 64.0;
const TILE_R: f64 = 20.0;
const CONTROL_BUTTON: f64 = 30.0;
/// Inside the controls tile, between its edge and its buttons.
const CONTROL_INSET: f64 = 10.0;
/// The next event, along the bottom of the weather tile.
const NEXT_EVENT_H: f64 = 38.0;
const CONTROL_GAP: f64 = 6.0;
/// Notifications shown at once, sharing the height of the list equally.
const VISIBLE_NOTIFICATIONS: f64 = 3.0;
const ICON: f64 = 46.0;
/// Shown while a tile's content is thinner than the tile.
const MIN_FILLER: f64 = 44.0;

type TrayAction = Box<dyn Fn(Vec<PathBuf>, &EngineHandle)>;
type Action = Box<dyn Fn()>;
/// A button's title and what it does.
type Labelled = (&'static str, Action);
type Run<'a> = (&'a str, &'a NSFont, &'a NSColor);

/// Pages of the expanded panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Planner,
    Media,
    Live,
    Agents,
}

impl Tab {
    /// The tab `step` places along the tab bar, wrapping around (← / →).
    pub fn step(self, step: i32) -> Tab {
        let i = TABS.iter().position(|t| *t == self).unwrap_or(0) as i32;
        TABS[(i + step).rem_euclid(TABS.len() as i32) as usize]
    }

    /// The tab at a 1-based position (⌘1 … ⌘5).
    pub fn nth(n: usize) -> Option<Tab> {
        n.checked_sub(1).and_then(|i| TABS.get(i).copied())
    }
}

/// The tabs in the order of their buttons.
pub const TABS: [Tab; 5] = [Tab::Overview, Tab::Planner, Tab::Media, Tab::Live, Tab::Agents];
const TAB_W: f64 = 34.0;

pub struct ExpandedView {
    pub root: Retained<FlippedView>,
    overview: Retained<FlippedView>,
    pub media: MediaPage,
    pub usage: UsagePage,
    pub planner: super::planner::PlannerPage,
    pub live: super::live::LivePage,
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
    /// Calendar card: accounts + upcoming events (EventKit).
    calendar_card: Retained<FlippedView>,
    calendar_doc: Retained<FlippedView>,
    calendar_targets: Targets,
    calendar_last: RefCell<Option<CalendarState>>,
    /// The quick-reply bar: an overlay strip under the notch band.
    reply_bar: Retained<FlippedView>,
    reply_label: Retained<NSTextField>,
    reply_field: Retained<NSTextField>,
    /// Keeps the Send pill's action target alive.
    _reply_targets: Targets,

    date_top: Retained<NSTextField>,
    now_pill: Retained<FlippedView>,
    weather_pill: Retained<FlippedView>,
    agents_pill: Retained<FlippedView>,
    /// The next event, on the Planner page.
    next_pill: Retained<FlippedView>,
    /// A call in progress, on every page.
    call_pill: Retained<FlippedView>,
    call_pill_text: Retained<NSTextField>,
    in_call: Cell<bool>,
    pill_right: f64,
    pill_max: f64,
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
    weather_width: f64,
    devices: Retained<FlippedView>,
    device_rings: RefCell<Vec<Ring>>,
    status_x: f64,
    status_y: f64,
    status_width: f64,
    status_h: f64,
    /// The height of each row of controls.
    control_h: f64,

    brightness: Retained<ThinSlider>,
    volume: Retained<ThinSlider>,
    mic_button: Retained<NSButton>,
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

/// The dark tint behind text on the glass, so it stays readable over anything.
fn ink(a: f64) -> Retained<NSColor> {
    rgba(0.07, 0.07, 0.094, a)
}

/// One widget tile on the glass.
pub fn glass_tile(mtm: MainThreadMarker, frame: NSRect) -> Retained<FlippedView> {
    let v = shape_view(mtm, frame, &ink(0.46), TILE_R);
    if let Some(l) = v.layer() {
        l.setBorderWidth(0.5);
        l.setBorderColor(Some(&white(0.18).CGColor()));
    }
    v
}

/// A small rounded label beside the notch, right-aligned at `right`.
fn pill(host: &FlippedView, right: f64, y: f64, width: f64) {
    host.setFrame(rect(right - width, y, width, 24.0));
    make_card(host, &white(0.12), 12.0);
}

/// A battery ring: name, level and how to draw it.
#[derive(Clone, PartialEq)]
struct Ring {
    name: String,
    level: u8,
    symbol: String,
}

/// One ring per device, with an AirPods-style case as its own ring.
fn battery_rings(devices: &[notchforge_core::model::DeviceBattery]) -> Vec<Ring> {
    let mut rings = Vec::new();
    for d in devices {
        let main = d.levels.iter().filter(|(c, _)| *c != BatteryComponent::Case).map(|(_, l)| *l).min();
        if let Some(level) = main {
            rings.push(Ring { name: d.name.clone(), level, symbol: d.kind.symbol().to_owned() });
        }
        if let Some((_, level)) = d.levels.iter().find(|(c, _)| *c == BatteryComponent::Case) {
            rings.push(Ring {
                name: "Case".into(),
                level: *level,
                symbol: if crate::ui::symbol_image("airpods.chargingcase.fill", 14.0, "").is_some() {
                    "airpods.chargingcase.fill".to_owned()
                } else {
                    d.kind.symbol().to_owned()
                },
            });
        }
    }
    rings.truncate(3);
    rings
}

/// A circular gauge filled clockwise from the top.
pub fn ring_view(mtm: MainThreadMarker, frame: NSRect, fraction: f64, color: &NSColor) -> Retained<FlippedView> {
    use objc2_core_graphics::CGMutablePath as P;
    use objc2_quartz_core::CAShapeLayer;
    let v = FlippedView::new(mtm, frame);
    v.setWantsLayer(true);
    let side = frame.size.width;
    let stroke = (side * 0.1).max(3.0);
    let r = (side - stroke) / 2.0;
    let path = P::new();
    let start = -std::f64::consts::FRAC_PI_2;
    // SAFETY: `path` is a valid mutable path; the transform is optional (null).
    unsafe {
        P::add_arc(
            Some(&path),
            std::ptr::null(),
            side / 2.0,
            side / 2.0,
            r,
            start,
            start + std::f64::consts::TAU,
            false,
        );
    }
    let layer = |c: &NSColor, end: f64| {
        let l = CAShapeLayer::layer();
        l.setFrame(rect(0.0, 0.0, side, side));
        l.setPath(Some(&path));
        l.setFillColor(None);
        l.setStrokeColor(Some(&c.CGColor()));
        l.setLineWidth(stroke);
        // SAFETY: "round" is kCALineCapRound.
        l.setLineCap(&crate::ui::ns("round"));
        l.setStrokeEnd(end);
        l
    };
    if let Some(root) = v.layer() {
        root.addSublayer(&layer(&white(0.18), 1.0));
        if fraction > 0.0 {
            root.addSublayer(&layer(color, fraction.min(1.0)));
        }
    }
    v
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
        let ty = ((nh - 24.0) / 2.0).max(2.0);

        // ── top row: page switch and clock (left), now playing and battery (right) ──
        let side = (w - layout.notch.rect.width) / 2.0;
        let date_top = label(
            mtm,
            "",
            &mono_digits(13.0, Weight::Semibold),
            &p.primary,
            rect(138.0, ty + 3.0, (side - 144.0).max(40.0), 18.0),
        );
        let battery_top = FlippedView::new(mtm, rect(w - 18.0 - 72.0, ty, 72.0, 24.0));
        root.addSubview(&battery_top);

        // Overview content lives in its own container so the music page can
        // replace it; both share the root's coordinate space.
        let overview = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        root.addSubview(&overview);

        let body_y = nh + 6.0;
        let body_h = h - body_y - PAD;
        // Four modules across, two down: notifications take a 2 × 2 block, and
        // weather, controls, the tray and batteries one module each.
        let module_w = (w - 2.0 * PAD - 3.0 * GAP) / 4.0;
        let c1w = 2.0 * module_w + GAP;
        let c2w = module_w;
        let c3w = module_w;
        let half_h = (body_h - GAP) / 2.0;
        let c1x = PAD;
        let c2x = c1x + c1w + GAP;
        let c3x = c2x + c2w + GAP;

        // ── notifications ──
        let notif_header = label(
            mtm,
            "Notifications",
            &font(15.0, Weight::Semibold),
            &p.primary,
            rect(c1x + 6.0, body_y + 3.0, c1w - 40.0, 19.0),
        );
        overview.addSubview(&notif_header);
        let eng = engine.clone();
        let t = targets.add(mtm, move |_| eng.send(Command::ClearNotifications));
        let clear = symbol_button(
            mtm,
            "xmark",
            9.0,
            "Clear all notifications",
            &p.primary,
            &t,
            rect(c1x + c1w - 26.0, body_y, 24.0, 24.0),
        );
        make_card(&clear, &white(0.14), 12.0);
        overview.addSubview(&clear);
        let list_h = body_h - 30.0;
        let notif_doc = FlippedView::new(mtm, rect(0.0, 0.0, c1w, list_h));
        let notif_scroll = scroll_view(mtm, rect(c1x, body_y + 30.0, c1w, list_h), &notif_doc);
        overview.addSubview(&notif_scroll);

        // ── weather, with the next event along its bottom ──
        let weather_h = half_h;
        let weather_card = FlippedView::new(mtm, rect(c2x, body_y, c2w, weather_h));
        weather_card.setWantsLayer(true);
        if let Some(l) = weather_card.layer() {
            l.setCornerRadius(TILE_R);
            l.setMasksToBounds(true);
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&white(0.22).CGColor()));
            let g = crate::ui::gradient_layer(
                &[(0.090, 0.251, 0.498, 1.0), (0.204, 0.424, 0.690, 1.0)],
                NSPoint::new(0.5, 0.0),
                NSPoint::new(0.5, 1.0),
            );
            g.setFrame(rect(0.0, 0.0, c2w, weather_h));
            l.addSublayer(&g);
        }
        overview.addSubview(&weather_card);

        // ── calendar: the next event or reminder, over the weather's hourly row ──
        let calendar_card = FlippedView::new(mtm, rect(c2x, body_y + weather_h - NEXT_EVENT_H, c2w, NEXT_EVENT_H));
        let calendar_doc = FlippedView::new(mtm, rect(0.0, 0.0, c2w, NEXT_EVENT_H));
        calendar_card.addSubview(&calendar_doc);
        overview.addSubview(&calendar_card);

        // ── tray ──
        let tray_y = body_y + half_h + GAP;
        let tray_h = half_h;
        let tray_box = glass_tile(mtm, rect(c2x, tray_y, c2w, tray_h));
        let tray_header =
            label(mtm, "Tray", &font(12.0, Weight::Semibold), &p.primary, rect(12.0, 9.0, c2w - 24.0, 15.0));
        tray_box.addSubview(&tray_header);
        let scroll_h = TILE_H + 4.0;
        let scroll_y = 28.0 + ((tray_h - 28.0 - 32.0 - scroll_h) / 2.0).max(0.0);
        let tray_w = c2w - 12.0;
        let tray_doc = FlippedView::new(mtm, rect(0.0, 0.0, tray_w, scroll_h));
        let tray_scroll = scroll_view(mtm, rect(6.0, scroll_y, tray_w, scroll_h), &tray_doc);
        tray_box.addSubview(&tray_scroll);
        let tray_empty = FlippedView::new(mtm, rect(6.0, scroll_y, tray_w, scroll_h));
        tray_empty.addSubview(&symbol_view(
            mtm,
            "tray.and.arrow.down",
            22.0,
            &p.secondary,
            rect((tray_w - 30.0) / 2.0, scroll_h / 2.0 - 26.0, 30.0, 26.0),
            "Drop files",
        ));
        let hint = label(
            mtm,
            "Drop files here",
            &font(12.0, Weight::Regular),
            &p.secondary,
            rect(0.0, scroll_h / 2.0 + 4.0, tray_w, 16.0),
        );
        hint.setAlignment(NSTextAlignment::Center);
        tray_empty.addSubview(&hint);
        tray_box.addSubview(&tray_empty);
        overview.addSubview(&tray_box);

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
        let step = (c2w - 12.0) / actions.len() as f64;
        for (i, (sym, tip, action)) in actions.into_iter().enumerate() {
            let eng = engine.clone();
            let t = targets.add(mtm, move |_| {
                let paths = with_controller(|c| c.tray_paths()).unwrap_or_default();
                action(paths, &eng);
            });
            tray_box.addSubview(&symbol_button(
                mtm,
                sym,
                13.0,
                tip,
                &p.primary,
                &t,
                rect(6.0 + i as f64 * step + (step - 32.0) / 2.0, tray_h - 28.0 - 3.0, 32.0, 28.0),
            ));
        }

        // ── Control Center modules ──
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
        // The controls share one tile, the batteries' twin below it.
        overview.addSubview(&glass_tile(mtm, rect(c3x, body_y, c3w, half_h)));
        let inner_x = c3x + CONTROL_INSET;
        let inner_w = c3w - 2.0 * CONTROL_INSET;
        let cell = inner_w / 4.0;
        let mut made = Vec::new();
        for (i, (sym, tip, action)) in buttons.into_iter().enumerate() {
            let t = targets.add(mtm, move |_| action());
            let b = symbol_button(
                mtm,
                sym,
                13.0,
                tip,
                &p.primary,
                &t,
                rect(
                    inner_x + i as f64 * cell + (cell - CONTROL_BUTTON) / 2.0,
                    body_y + CONTROL_INSET,
                    CONTROL_BUTTON,
                    CONTROL_BUTTON,
                ),
            );
            make_card(&b, &white(0.14), CONTROL_BUTTON / 2.0);
            overview.addSubview(&b);
            made.push(b);
        }
        let focus_button = made[0].clone();
        let pause_button = made[1].clone();

        // Under the buttons, up to three rows share what the tile has left.
        let rows_top = body_y + CONTROL_INSET + CONTROL_BUTTON + CONTROL_GAP;
        let rows_h = body_y + half_h - CONTROL_INSET - rows_top;
        let control_h = ((rows_h - 2.0 * CONTROL_GAP) / 3.0).clamp(22.0, 32.0);
        let slider = |sym: &str, tip: &str, set: fn(f64)| {
            let s = ThinSlider::capsule(mtm, rect(c3x, body_y, c3w, control_h), control_h, &white(0.12), &white(0.94));
            s.addSubview(&symbol_view(
                mtm,
                sym,
                13.0,
                &rgba(0.11, 0.11, 0.118, 1.0),
                rect(11.0, (control_h - 16.0) / 2.0, 16.0, 16.0),
                tip,
            ));
            set_accessibility_label(&s, tip);
            s.set_on_change(set);
            overview.addSubview(&s);
            s
        };
        let brightness_slider = slider("sun.max.fill", "Display brightness", |v| {
            brightness::set(v as f32);
        });
        let volume_slider = slider("speaker.wave.1.fill", "Output volume", |v| {
            volume::set(v as f32);
        });
        // Microphone mute sits at the trailing edge of the volume row.
        let t = targets.add(mtm, move |_| {
            with_controller(|c| c.toggle_mic());
        });
        let mic_button = symbol_button(
            mtm,
            "mic.fill",
            13.0,
            "Microphone mute",
            &p.primary,
            &t,
            rect(0.0, 0.0, CONTROL_BUTTON, CONTROL_BUTTON),
        );
        make_card(&mic_button, &white(0.14), CONTROL_BUTTON / 2.0);
        set_accessibility_label(&mic_button, "Microphone mute");
        overview.addSubview(&mic_button);

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
            &ink(0.46),
            control_h / 2.0,
            &t,
            rect(c3x, body_y, c3w, control_h),
        );
        resolution.setAlignment(NSTextAlignment::Left);
        resolution.addSubview(&symbol_view(
            mtm,
            "chevron.up.chevron.down",
            9.0,
            &p.secondary,
            rect(inner_w - 24.0, (control_h - 12.0) / 2.0, 12.0, 12.0),
            "",
        ));
        set_accessibility_label(&resolution, "Display resolution");
        overview.addSubview(&resolution);

        let devices = glass_tile(mtm, rect(c3x, body_y + half_h + GAP, c3w, half_h));
        overview.addSubview(&devices);

        // ── music page + top-row pills + tab buttons (added last so they sit on top) ──
        let media = MediaPage::new(mtm, w, h, nh + 8.0, p);
        media.root.setHidden(true);
        root.addSubview(&media.root);
        let usage = UsagePage::new(mtm, w, h, nh, p);
        usage.root.setHidden(true);
        root.addSubview(&usage.root);
        let data_dir =
            notchforge_core::config::Paths::resolve().map(|p| p.data_dir).unwrap_or_else(|_| std::env::temp_dir());
        let planner = super::planner::PlannerPage::new(mtm, w, h, nh, p, data_dir);
        planner.root.setHidden(true);
        root.addSubview(&planner.root);
        let live = super::live::LivePage::new(mtm, w, h, nh, p);
        live.root.setHidden(true);
        root.addSubview(&live.root);
        // Five tabs leave no room for the date beside the notch: it's on the Planner.
        date_top.setHidden(true);
        root.addSubview(&date_top);
        let now_pill = FlippedView::new(mtm, rect(0.0, ty, 0.0, 24.0));
        let weather_pill = FlippedView::new(mtm, rect(0.0, ty, 0.0, 24.0));
        root.addSubview(&now_pill);
        root.addSubview(&weather_pill);
        let agents_pill = FlippedView::new(mtm, rect(0.0, ty, 0.0, 24.0));
        root.addSubview(&agents_pill);
        let next_pill = FlippedView::new(mtm, rect(0.0, ty, 0.0, 24.0));
        root.addSubview(&next_pill);
        let call_w = (side - 72.0 - 18.0 - 16.0).max(60.0);
        let call_pill = FlippedView::new(mtm, rect(w - 18.0 - 72.0 - 8.0 - call_w, ty, call_w, 24.0));
        make_card(&call_pill, &rgba(0.141, 0.541, 0.239, 1.0), 12.0);
        call_pill.addSubview(&symbol_view(
            mtm,
            "phone.fill",
            11.0,
            &p.primary,
            rect(8.0, 5.0, 14.0, 14.0),
            "On a call",
        ));
        let call_pill_text = label(
            mtm,
            "",
            &mono_digits(12.0, Weight::Semibold),
            &p.primary,
            rect(25.0, 4.0, call_w - 25.0 - 26.0, 16.0),
        );
        call_pill.addSubview(&call_pill_text);
        let t = targets.add(mtm, |_| {
            with_controller(|c| c.hang_up());
        });
        let hang_up = symbol_button(
            mtm,
            "phone.down.fill",
            9.0,
            "Hang up",
            &NSColor::whiteColor(),
            &t,
            rect(call_w - 22.0, 2.0, 20.0, 20.0),
        );
        make_card(&hang_up, &rgba(0.851, 0.176, 0.125, 1.0), 10.0);
        call_pill.addSubview(&hang_up);
        call_pill.setHidden(true);
        root.addSubview(&call_pill);
        let track = shape_view(mtm, rect(PAD, ty - 2.0, TABS.len() as f64 * TAB_W + 4.0, 28.0), &white(0.12), 14.0);
        root.addSubview(&track);
        let tabs: [(&str, &str, Tab); 5] = [
            ("square.grid.2x2.fill", "Overview", Tab::Overview),
            ("calendar", "Planner", Tab::Planner),
            ("music.note", "Now Playing", Tab::Media),
            ("waveform.path.ecg", "Live sessions", Tab::Live),
            ("chart.bar.fill", "Usage", Tab::Agents),
        ];
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
                    &p.secondary,
                    &t,
                    rect(PAD + 2.0 + i as f64 * TAB_W, ty, TAB_W, 24.0),
                );
                root.addSubview(&b);
                b
            })
            .collect();

        // ── quick reply bar: an overlay under the notch band ──
        let reply_bar = FlippedView::new(mtm, rect(PAD, layout.notch.rect.height + 6.0, w - 2.0 * PAD, 48.0));
        reply_bar.setWantsLayer(true);
        if let Some(l) = reply_bar.layer() {
            l.setBackgroundColor(Some(&rgba(0.09, 0.09, 0.11, 0.98).CGColor()));
            l.setCornerRadius(14.0);
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&rgba(1.0, 1.0, 1.0, 0.14).CGColor()));
        }
        let reply_label = label(mtm, "", &font(12.0, Weight::Semibold), &p.primary, rect(14.0, 6.0, 240.0, 16.0));
        reply_bar.addSubview(&reply_label);
        let reply_targets = Targets::default();
        let rt = reply_targets.add(mtm, move |_| {
            crate::app::with_controller(|c| c.send_reply());
        });
        let reply_field =
            NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(14.0, 27.0, w - 2.0 * PAD - 96.0, 17.0));
        reply_field.setEditable(true);
        reply_field.setBordered(false);
        reply_field.setBezeled(true);
        reply_field.setBezelStyle(objc2_app_kit::NSTextFieldBezelStyle::SquareBezel);
        reply_field.setFont(Some(&font(13.0, Weight::Regular)));
        reply_field.setPlaceholderString(Some(&crate::ui::ns("Type your reply, ⏎ to send")));
        // SAFETY: a valid target and selector; the target outlives the field.
        unsafe {
            reply_field.setTarget(Some(rt.as_any()));
            reply_field.setAction(Some(crate::ui::ActionTarget::action()));
        }
        reply_bar.addSubview(&reply_field);
        // Canned replies: one tap drops the common answer into the field.
        for (i, canned) in ["👍", "🙏", "✅"].iter().enumerate() {
            let tip = format!("Quick reply: {canned}");
            let canned = canned.to_string();
            let t = reply_targets.add(mtm, move |_| {
                crate::app::with_controller(|c| {
                    let field = c.reply_field_for_canned();
                    let current = field.stringValue().to_string();
                    crate::ui::set_text(&field, &format!("{current}{canned}"));
                });
            });
            let chip = symbol_button(
                mtm,
                match i {
                    0 => "hand.thumbsup.fill",
                    1 => "hand.wave.fill",
                    _ => "checkmark.circle.fill",
                },
                11.0,
                &tip,
                &p.secondary,
                &t,
                rect(258.0 + i as f64 * 26.0, 5.0, 22.0, 18.0),
            );
            reply_bar.addSubview(&chip);
        }
        let send = pill_button(
            mtm,
            "Send",
            None,
            &font(12.5, Weight::Semibold),
            &p.primary,
            &rgba(0.039, 0.518, 1.0, 1.0),
            14.0,
            &rt,
            rect(w - 2.0 * PAD - 76.0, 9.0, 62.0, 30.0),
        );
        reply_bar.addSubview(&send);
        reply_bar.setHidden(true);

        Self {
            root,
            overview,
            media,
            usage,
            planner,
            live,
            tab: Cell::new(Tab::Overview),
            tab_buttons,
            visible: Cell::new(false),
            mtm,
            palette,
            engine,
            config: RefCell::new(config),
            _targets: targets,
            calendar_card,
            calendar_doc,
            calendar_targets: Targets::default(),
            calendar_last: RefCell::new(None),
            reply_bar,
            reply_label,
            reply_field,
            _reply_targets: reply_targets,
            row_targets: Targets::default(),
            date_top,
            now_pill,
            weather_pill,
            agents_pill,
            next_pill,
            call_pill,
            call_pill_text,
            in_call: Cell::new(false),
            pill_right: w - 18.0 - 72.0 - 8.0,
            pill_max: (side - 72.0 - 18.0 - 16.0).max(60.0),
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
            tray_width: tray_w,
            tray_paths: RefCell::new(Vec::new()),
            weather_card,
            weather_width: c2w,
            devices,
            device_rings: RefCell::new(Vec::new()),
            status_x: inner_x,
            status_y: rows_top,
            status_width: inner_w,
            status_h: rows_h,
            control_h,
            brightness: brightness_slider,
            volume: volume_slider,
            mic_button,
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
        self.usage.root.setHidden(tab != Tab::Agents);
        self.planner.root.setHidden(tab != Tab::Planner);
        self.live.root.setHidden(tab != Tab::Live);
        if previous == Tab::Planner {
            self.planner.save_note();
        }
        for (b, t) in self.tab_buttons.iter().zip(TABS) {
            let on = t == tab;
            b.setContentTintColor(Some(if on { &self.palette.primary } else { &self.palette.tertiary }));
            // Selected segment: a lighter pill inside the shared track.
            make_card(b, &white(if on { 0.28 } else { 0.0 }), 12.0);
        }
        self.sync_pills();
        self.media.set_active(self.visible.get() && tab == Tab::Media);
        if tab == Tab::Media && previous != Tab::Media {
            crate::media::send(notchforge_core::provider::ProviderCommand::MediaChanged(None));
            crate::media::send(notchforge_core::provider::ProviderCommand::ReloadPlaylists);
        }
    }

    /// The panel opened or closed.
    pub fn set_visible(&self, visible: bool) {
        self.visible.set(visible);
        if visible {
            self.render_clock();
        }
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
                layer.setBorderColor(Some(&white(0.18).CGColor()));
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
        self.usage.update(&snap.usage, self.config.borrow().agents.token_usage);
        self.planner.update(snap);
        self.live.update(snap);
        self.render_notifications(snap);
        self.render_tray(snap);
        self.render_weather(snap);
        self.render_devices(snap);
        self.render_calendar(snap);

        let p = &self.palette;
        let focus_on = snap.focus.active == Some(true);
        let focus_bg = if focus_on { rgba(0.368, 0.361, 0.902, 1.0) } else { white(0.14) };
        make_card(&self.focus_button, &focus_bg, CONTROL_BUTTON / 2.0);
        self.focus_button.setContentTintColor(Some(&p.primary));
        let pause_bg = if snap.paused { rgba(1.0, 0.62, 0.04, 1.0) } else { white(0.14) };
        make_card(&self.pause_button, &pause_bg, CONTROL_BUTTON / 2.0);
        self.pause_button.setContentTintColor(Some(&p.primary));
        set_accessibility_label(
            &self.pause_button,
            if snap.paused { "Resume notifications" } else { "Pause notifications" },
        );
    }

    /// Only one pill beside the notch: a call in progress, else the page's own.
    fn sync_pills(&self) {
        let (in_call, tab) = (self.in_call.get(), self.tab.get());
        self.call_pill.setHidden(!in_call);
        self.now_pill.setHidden(in_call || tab != Tab::Overview || self.now_pill.subviews().is_empty());
        self.weather_pill.setHidden(in_call || tab != Tab::Media || self.weather_pill.subviews().is_empty());
        let agents = matches!(tab, Tab::Agents | Tab::Live);
        self.agents_pill.setHidden(in_call || !agents || self.agents_pill.subviews().is_empty());
        self.next_pill.setHidden(in_call || tab != Tab::Planner || self.next_pill.subviews().is_empty());
    }

    /// Show the call in progress, or that there isn't one. Called every second.
    pub fn set_call(&self, call: Option<&notchforge_core::calls::ActiveCall>, now_ms: i64) {
        if let Some(c) = call {
            let clock = notchforge_core::calls::call_clock(c.started_ms, now_ms);
            crate::ui::set_text(&self.call_pill_text, &format!("{clock} · {}", c.caller));
        }
        self.in_call.set(call.is_some());
        self.sync_pills();
    }

    /// The date and time beside the page switch.
    fn render_clock(&self) {
        let f = objc2_foundation::NSDateFormatter::new();
        let now = objc2_foundation::NSDate::now();
        f.setLocalizedDateFormatFromTemplate(&crate::ui::ns("MMMd"));
        let date = f.stringFromDate(&now);
        f.setLocalizedDateFormatFromTemplate(&crate::ui::ns("HH:mm"));
        let time = f.stringFromDate(&now);
        crate::ui::set_text(&self.date_top, &format!("{date} {time}"));
    }

    fn render_top(&self, snap: &Snapshot) {
        let p = &self.palette;
        let mtm = self.mtm;
        self.render_clock();

        // What's playing, on the Overview page.
        remove_all_subviews(&self.now_pill);
        if let Some(n) = snap.media.primary() {
            let f = font(12.0, Weight::Semibold);
            let mut text = n.title.clone();
            if !n.artist.is_empty() {
                text.push_str(" · ");
                text.push_str(&n.artist);
            }
            let tw = crate::ui::text_width(&crate::ui::attributed(&text, &f, &p.primary));
            let width = (tw + 39.0).min(self.pill_max);
            let y = self.now_pill.frame().origin.y;
            pill(&self.now_pill, self.pill_right, y, width);
            let brand = crate::ui::rgb8(n.player.color());
            self.now_pill.addSubview(&super::media::equalizer_view(mtm, rect(9.0, 6.5, 12.0, 11.0), &brand));
            self.now_pill.addSubview(&label(mtm, &text, &f, &p.primary, rect(27.0, 4.0, width - 35.0, 16.0)));
            set_accessibility_label(&self.now_pill, &format!("Now playing: {text}"));
        }
        // The weather, on the Now Playing page.
        remove_all_subviews(&self.weather_pill);
        if let Some(w) = &snap.weather {
            let f = mono_digits(12.0, Weight::Semibold);
            let text = format!("{} {}", temperature(w.temperature), w.place);
            let tw = crate::ui::text_width(&crate::ui::attributed(&text, &f, &p.primary));
            let width = (tw + 39.0).min(self.pill_max);
            let y = self.weather_pill.frame().origin.y;
            pill(&self.weather_pill, self.pill_right, y, width);
            let icon = symbol_view(
                mtm,
                w.condition.symbol(w.is_day),
                12.0,
                &p.primary,
                rect(8.0, 5.0, 16.0, 14.0),
                w.condition.label(),
            );
            crate::views::multicolor(&icon);
            self.weather_pill.addSubview(&icon);
            self.weather_pill.addSubview(&label(mtm, &text, &f, &p.primary, rect(27.0, 4.0, width - 35.0, 16.0)));
        }
        // Running sessions, on the Agents page.
        remove_all_subviews(&self.agents_pill);
        if let Some(text) = UsagePage::running_label(snap.usage.running.iter().filter(|r| r.working).count()) {
            let f = font(12.0, Weight::Semibold);
            let tw = crate::ui::text_width(&crate::ui::attributed(&text, &f, &p.primary));
            let width = (tw + 34.0).min(self.pill_max);
            let y = self.agents_pill.frame().origin.y;
            pill(&self.agents_pill, self.pill_right, y, width);
            self.agents_pill.addSubview(&shape_view(mtm, rect(9.0, 8.5, 7.0, 7.0), &p.success, 3.5));
            self.agents_pill.addSubview(&label(mtm, &text, &f, &p.primary, rect(22.0, 4.0, width - 30.0, 16.0)));
        }
        // The next event, on the Planner page.
        remove_all_subviews(&self.next_pill);
        if let Some(e) = snap.calendar.events.iter().find(|e| e.starts_at_ms > snap.now_ms && !e.all_day) {
            let f = font(12.0, Weight::Semibold);
            let mins = (e.starts_at_ms - snap.now_ms) / 60_000;
            let when = if mins < 60 { format!("in {} min", mins.max(1)) } else { format!("in {}h", (mins + 30) / 60) };
            let text = format!("Next: {} {when}", e.title);
            let tw = crate::ui::text_width(&crate::ui::attributed(&text, &f, &p.primary));
            let width = (tw + 34.0).min(self.pill_max);
            let y = self.next_pill.frame().origin.y;
            pill(&self.next_pill, self.pill_right, y, width);
            let (r, g, b) = e.color.unwrap_or((10, 132, 255));
            let dot = rgba(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, 1.0);
            self.next_pill.addSubview(&shape_view(mtm, rect(9.0, 8.5, 7.0, 7.0), &dot, 3.5));
            self.next_pill.addSubview(&label(mtm, &text, &f, &p.primary, rect(22.0, 4.0, width - 30.0, 16.0)));
        }
        self.sync_pills();

        remove_all_subviews(&self.battery_top);
        if let Some(b) = snap.battery.as_ref().filter(|b| b.present) {
            let bw = self.battery_top.frame().size.width;
            let l = label(
                mtm,
                &format!("{}%", b.percent),
                &mono_digits(13.0, Weight::Semibold),
                &p.primary,
                rect(0.0, 3.0, bw - 30.0, 18.0),
            );
            l.setAlignment(NSTextAlignment::Right);
            self.battery_top.addSubview(&l);
            self.battery_top.addSubview(&battery_glyph(mtm, rect(bw - 26.0, 6.0, 26.0, 12.0), b, p));
            set_accessibility_label(&self.battery_top, &super::battery_spoken(b));
        }
    }

    fn render_weather(&self, snap: &Snapshot) {
        let p = &self.palette;
        let mtm = self.mtm;
        let wc = self.weather_width;
        let wh = self.weather_card.frame().size.height;
        remove_all_subviews(&self.weather_card);
        let light = font(32.0, Weight::Light);
        let Some(w) = &snap.weather else {
            let temp = label(mtm, "--°", &light, &p.primary, rect(12.0, 12.0, wc - 24.0, 46.0));
            self.weather_card.addSubview(&temp);
            let hint = match snap.providers.get("weather") {
                Some(notchforge_core::model::ProviderStatus::NeedsPermission(_)) => "Set a location in Settings",
                Some(notchforge_core::model::ProviderStatus::Disabled) => "Weather is off",
                _ => "Weather unavailable",
            };
            let l = label(mtm, hint, &font(12.0, Weight::Regular), &p.primary, rect(12.0, 62.0, wc - 24.0, 16.0));
            self.weather_card.addSubview(&l);
            return;
        };
        self.weather_card.addSubview(&label(
            mtm,
            &w.place,
            &font(13.0, Weight::Semibold),
            &p.primary,
            rect(12.0, 10.0, wc - 56.0, 16.0),
        ));
        let icon = symbol_view(
            mtm,
            w.condition.symbol(w.is_day),
            20.0,
            &p.primary,
            rect(wc - 40.0, 10.0, 28.0, 24.0),
            w.condition.label(),
        );
        crate::views::multicolor(&icon);
        self.weather_card.addSubview(&icon);
        // The temperature on the left; what it's like on the right, under the icon.
        self.weather_card.addSubview(&label(
            mtm,
            &temperature(w.temperature),
            &light,
            &p.primary,
            rect(12.0, 24.0, wc / 2.0, 40.0),
        ));
        // Beside the temperature, the day's range; under it, what it's like.
        let line = font(11.5, Weight::Regular);
        let range = label(
            mtm,
            &format!("H:{} L:{}", temperature(w.high), temperature(w.low)),
            &line,
            &p.primary,
            rect(wc / 2.0, 42.0, wc / 2.0 - 12.0, 15.0),
        );
        range.setAlignment(NSTextAlignment::Right);
        self.weather_card.addSubview(&range);
        self.weather_card.addSubview(&label(
            mtm,
            w.condition.label(),
            &line,
            &p.primary,
            rect(12.0, 64.0, wc - 24.0, 15.0),
        ));
        // Sun times and air quality fill the corner when they exist.
        let extras = {
            let mut parts = Vec::new();
            if let Some(rise) = &w.sunrise {
                parts.push(format!("↑{rise}"));
            }
            if let Some(set) = &w.sunset {
                parts.push(format!("↓{set}"));
            }
            if let Some(aqi) = w.aqi {
                let word = match aqi {
                    0..=20 => "Air excellent",
                    21..=40 => "Air good",
                    41..=60 => "Air moderate",
                    61..=80 => "Air poor",
                    _ => "Air very poor",
                };
                parts.push(word.to_owned());
            }
            parts.join("  ·  ")
        };
        if !extras.is_empty() {
            self.weather_card.addSubview(&label(
                mtm,
                &extras,
                &font(10.5, Weight::Regular),
                &p.secondary,
                rect(wc / 2.0, 64.0, wc / 2.0 - 12.0, 15.0),
            ));
        }
        // The next event takes the bottom row when there is one.
        let cal = &snap.calendar;
        let next_event = cal.granted && (!cal.events.is_empty() || !cal.reminders.is_empty());
        let n = if next_event { 0 } else { 4usize.min(w.hourly.len() + 1) };
        let cw = (wc - 16.0) / 4.0;
        let hy = wh - 50.0;
        for i in 0..n {
            let (hour, cond, t) = if i == 0 {
                ("Now".to_owned(), w.condition.symbol(w.is_day), w.temperature)
            } else {
                let hf = &w.hourly[i - 1];
                // Night icons (moon) outside 06:00–19:59, as on the board.
                let night = hf.hour.parse::<u32>().is_ok_and(|h| !(6..20).contains(&h));
                (hf.hour.clone(), hf.condition.symbol(!night), hf.temperature)
            };
            let x = 8.0 + i as f64 * cw;
            let hl = label(mtm, &hour, &font(11.0, Weight::Regular), &p.primary, rect(x, hy, cw, 14.0));
            hl.setAlignment(NSTextAlignment::Center);
            let ic = symbol_view(mtm, cond, 11.0, &p.primary, rect(x + (cw - 16.0) / 2.0, hy + 15.0, 16.0, 13.0), "");
            crate::views::multicolor(&ic);
            let tl = label(
                mtm,
                &temperature(t),
                &mono_digits(11.0, Weight::Semibold),
                &p.primary,
                rect(x, hy + 28.0, cw, 14.0),
            );
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
        let rings = battery_rings(&snap.devices);
        *self.device_rings.borrow_mut() = rings;
        self.draw_batteries();
    }

    /// Fit the battery rings to their tile, which takes whatever height the
    /// controls above it leave.
    fn draw_batteries(&self) {
        let p = &self.palette;
        let mtm = self.mtm;
        remove_all_subviews(&self.devices);
        let size = self.devices.frame().size;
        let rings = self.device_rings.borrow();
        self.devices.addSubview(&label(
            mtm,
            "Batteries",
            &font(12.0, Weight::Semibold),
            &p.primary,
            rect(12.0, 9.0, size.width - 24.0, 15.0),
        ));
        if rings.is_empty() {
            let l = wrapping_label(
                mtm,
                "No accessories reporting battery",
                &font(12.0, Weight::Regular),
                &p.secondary,
                rect(14.0, 30.0, size.width - 28.0, (size.height - 38.0).max(16.0)),
                3,
            );
            l.setAlignment(NSTextAlignment::Center);
            self.devices.addSubview(&l);
            return;
        }
        let cell = (size.width - 12.0) / 3.0;
        let side = (size.height - 28.0 - 42.0).clamp(26.0, (cell - 6.0).min(60.0));
        // Centre the rings when there are fewer than three.
        let x0 = 6.0 + (3 - rings.len()) as f64 * cell / 2.0;
        for (i, r) in rings.iter().enumerate() {
            let x = x0 + i as f64 * cell;
            let low = r.level <= 20;
            let color = if low { p.warning.clone() } else { rgba(0.188, 0.82, 0.345, 1.0) };
            let top = 26.0 + (size.height - 26.0 - 6.0 - side - 33.0) / 2.0;
            let ring = ring_view(mtm, rect(x + (cell - side) / 2.0, top, side, side), r.level as f64 / 100.0, &color);
            let g = side * 0.42;
            ring.addSubview(&symbol_view(
                mtm,
                &r.symbol,
                g * 0.8,
                &p.primary,
                rect((side - g) / 2.0, (side - g) / 2.0, g, g),
                "",
            ));
            self.devices.addSubview(&ring);
            let pct = label(
                mtm,
                &format!("{}%", r.level),
                &mono_digits(13.0, Weight::Semibold),
                if low { &p.warning } else { &p.primary },
                rect(x, top + side + 3.0, cell, 16.0),
            );
            pct.setAlignment(NSTextAlignment::Center);
            self.devices.addSubview(&pct);
            let name = label(
                mtm,
                &r.name,
                &font(11.0, Weight::Regular),
                &white(0.74),
                rect(x + 2.0, top + side + 19.0, cell - 4.0, 14.0),
            );
            name.setAlignment(NSTextAlignment::Center);
            set_accessibility_label(&name, &format!("{} battery {}%", r.name, r.level));
            self.devices.addSubview(&name);
        }
    }

    /// Stack the controls that are in use; the batteries take what is left.
    fn layout_status(&self) {
        let (x, w) = (self.status_x, self.status_width);
        let rows: Vec<&NSView> = [&*self.brightness as &NSView, &*self.volume, &*self.resolution]
            .into_iter()
            .filter(|v| !v.isHidden())
            .collect();
        // Whichever rows are in use sit evenly under the buttons.
        let top = self.status_y;
        let used = rows.len() as f64 * self.control_h + rows.len().saturating_sub(1) as f64 * CONTROL_GAP;
        let mut y = top + ((self.status_h - used) / 2.0).max(0.0);
        let mic_on_volume_row = !self.volume.isHidden() && !self.mic_button.isHidden();
        for (s, mic_here) in [(&self.brightness, false), (&self.volume, mic_on_volume_row)] {
            if !s.isHidden() {
                // The volume row shares its trailing edge with the mic button.
                let side = self.control_h;
                let row_w = if mic_here { w - side - 6.0 } else { w };
                s.set_slider_frame(rect(x, y, row_w, self.control_h));
                if mic_here {
                    self.mic_button.setFrame(rect(x + w - side, y, side, side));
                }
                y += self.control_h + CONTROL_GAP;
            }
        }
        if !self.resolution.isHidden() {
            self.resolution.setFrame(rect(x, y, w, self.control_h));
        }
        self.draw_batteries();
    }

    fn render_notifications(&self, snap: &Snapshot) {
        let p = &self.palette;
        let mtm = self.mtm;
        remove_all_subviews(&self.notif_doc);
        let mut parts = vec![("Notifications".to_owned(), p.primary.clone())];
        if !snap.notifications.is_empty() {
            parts.push((format!("  {}", snap.notifications.len()), p.tertiary.clone()));
        }
        if snap.paused {
            parts.push(("  · paused".into(), p.warning.clone()));
        }
        let f = font(15.0, Weight::Semibold);
        self.notif_header.setAttributedStringValue(&attributed_parts(&runs(&parts, &f)));
        let visible_h = self.notif_scroll.frame().size.height;
        let w = self.notif_width;
        if snap.notifications.is_empty() {
            self.notif_doc.setFrame(rect(0.0, 0.0, w, visible_h));
            let lang = notchforge_core::i18n::Language::from_config(&self.config.borrow().general);
            self.notif_doc.addSubview(&self.quiet_tile(
                rect(0.0, 0.0, w, visible_h),
                Some("checkmark.circle"),
                notchforge_core::i18n::tr(lang, notchforge_core::i18n::Str::CaughtUp),
            ));
            return;
        }
        let rows: Vec<&Notification> = snap.notifications.iter().take(MAX_ROWS).collect();
        let fit = (visible_h - (VISIBLE_NOTIFICATIONS - 1.0) * ROW_GAP) / VISIBLE_NOTIFICATIONS;
        let mut heights: Vec<f64> = rows.iter().map(|n| Self::row_height(n).max(fit)).collect();
        // Only a stack's lip adds to the row height.
        let lips: Vec<f64> = rows.iter().map(|n| if n.group_count > 1 { 6.0 } else { 0.0 }).collect();
        let used: f64 = heights.iter().sum::<f64>() + lips.iter().sum::<f64>() + ROW_GAP * (rows.len() - 1) as f64;
        // A thin leftover goes to the last card; a larger one gets its own tile.
        let spare = visible_h - used;
        if spare > 0.0 && spare < MIN_FILLER + ROW_GAP {
            if let Some(last) = heights.last_mut() {
                *last += spare;
            }
        }
        let mut y = 0.0;
        let mut views: Vec<Retained<FlippedView>> = Vec::new();
        for (i, n) in rows.iter().enumerate() {
            let rh = heights[i];
            if lips[i] > 0.0 {
                let lip = shape_view(self.mtm, rect(12.0, y + rh - 24.0, w - 24.0, 30.0), &ink(0.3), 16.0);
                if let Some(l) = lip.layer() {
                    l.setBorderWidth(0.5);
                    l.setBorderColor(Some(&white(0.14).CGColor()));
                }
                views.push(lip);
            }
            views.push(self.notification_row(n, snap.now_ms, rect(0.0, y, w, rh)));
            y += rh + lips[i] + ROW_GAP;
        }
        if spare >= MIN_FILLER + ROW_GAP {
            views.push(self.quiet_tile(rect(0.0, y, w, visible_h - y), None, "No older notifications"));
            y = visible_h;
        }
        self.notif_doc.setFrame(rect(0.0, 0.0, w, (y - ROW_GAP).max(visible_h)));
        for v in views {
            self.notif_doc.addSubview(&v);
        }
        // The archive: what came through earlier, dimmed and quiet.
        let recent: Vec<&Notification> = snap.history.iter().take(4).collect();
        if recent.is_empty() {
            return;
        }
        let hy = y + 6.0;
        self.notif_doc.addSubview(&label(
            mtm,
            "History",
            &font(11.0, Weight::Semibold),
            &p.tertiary,
            rect(12.0, hy, w - 24.0, 14.0),
        ));
        let mut hy = hy + 18.0;
        for n in recent {
            let row = label(
                mtm,
                &format!("{} — {}", n.app.display_name, n.title),
                &font(11.5, Weight::Regular),
                &p.tertiary,
                rect(12.0, hy, w - 24.0, 14.0),
            );
            row.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            self.notif_doc.addSubview(&row);
            hy += 16.0;
        }
        self.notif_doc
            .setFrame(rect(0.0, 0.0, w, (hy - ROW_GAP).max(visible_h)));
    }

    /// A quiet tile holding a short message, so a short list leaves no hole.
    fn quiet_tile(&self, frame: NSRect, symbol: Option<&str>, text: &str) -> Retained<FlippedView> {
        let p = &self.palette;
        let tile = glass_tile(self.mtm, frame);
        let (w, h) = (frame.size.width, frame.size.height);
        let mut ly = h / 2.0 - 8.0;
        if let Some(s) = symbol {
            tile.addSubview(&symbol_view(
                self.mtm,
                s,
                26.0,
                &p.secondary,
                rect((w - 32.0) / 2.0, h / 2.0 - 34.0, 32.0, 32.0),
                text,
            ));
            ly = h / 2.0 + 4.0;
        }
        let l = label(self.mtm, text, &font(12.5, Weight::Medium), &p.secondary, rect(0.0, ly, w, 17.0));
        l.setAlignment(NSTextAlignment::Center);
        tile.addSubview(&l);
        tile
    }

    /// Board metrics: 12 pt padding, 16 + 17 (+ 15 body) pt of text, and a row
    /// of 28 pt buttons on an authorization card.
    fn row_height(n: &Notification) -> f64 {
        let text = if n.body.trim().is_empty() { 57.0 } else { 72.0 };
        if n.auth_target().is_some() { text + 38.0 } else { text }
    }

    fn notification_row(&self, n: &Notification, now_ms: i64, frame: NSRect) -> Retained<FlippedView> {
        let p = &self.palette;
        let mtm = self.mtm;
        let w = frame.size.width;
        let urgent =
            n.priority >= Priority::High || matches!(n.kind, NotificationKind::NeedsInput | NotificationKind::Error);
        let row = glass_tile(mtm, frame);
        // A row taller than its text keeps the text in the middle. Sign-in rows
        // have buttons along the bottom.
        let top = if n.auth_target().is_some() { 12.0 } else { ((frame.size.height - 48.0) / 2.0).max(10.0) };
        let icon = app_icon_view(mtm, rect(12.0, top, 34.0, 34.0));
        set_app_icon(&icon, &n.app);
        row.addSubview(&icon);

        let kind = match n.kind {
            NotificationKind::NeedsInput => Some("Needs input"),
            NotificationKind::TaskFinished => Some("Task finished"),
            NotificationKind::Error => Some("Error"),
            NotificationKind::Authorization => {
                Some(if matches!(n.auth_target(), Some(AuthTarget::Spotify)) { "Sign-in" } else { "Wants access" })
            }
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
        let head_color = if urgent { rgba(1.0, 0.70, 0.25, 1.0) } else { p.primary.clone() };
        let tx = 56.0;
        let body_color = white(0.76);
        let time = label(
            mtm,
            &relative(n.timestamp_ms, now_ms),
            &font(12.0, Weight::Regular),
            &body_color,
            rect(w - 46.0, top, 34.0, 16.0),
        );
        time.setAlignment(NSTextAlignment::Right);
        row.addSubview(&time);
        row.addSubview(&label(
            mtm,
            &head,
            &font(12.0, Weight::Semibold),
            &head_color,
            rect(tx, top, w - tx - 46.0, 16.0),
        ));
        let auth = n.auth_target();
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
            rect(tx, top + 16.0, w - tx - 12.0, 17.0),
        ));
        if !n.body.trim().is_empty() {
            row.addSubview(&label(
                mtm,
                &n.body.replace('\n', " "),
                &font(12.0, Weight::Regular),
                &body_color,
                rect(tx, top + 33.0, w - tx - 12.0, 15.0),
            ));
        }
        if let Some(target) = auth {
            set_accessibility_label(&row, &format!("{}, {}", n.plain_text(), relative_spoken(n.timestamp_ms, now_ms)));
            self.auth_buttons(&row, &target, w, frame.size.height);
            return row;
        }
        // Messaging messages can be answered from the notch.
        let replyable = self.config.borrow().notifications.quick_reply
            && matches!(n.kind, NotificationKind::Message)
            && n.app.category() == notchforge_core::apps::AppCategory::Messaging;
        if replyable {
            let id = n.id;
            let t = self.row_targets.add(mtm, move |_| {
                with_controller(|c| c.start_reply(id));
            });
            // Beside the title and message, which make room for it.
            let pill_w = 58.0;
            for v in row.subviews().iter() {
                let f = v.frame();
                if f.origin.y > top + 8.0 && f.origin.x + f.size.width > w - 12.0 - pill_w - 8.0 {
                    v.setFrameSize(objc2_foundation::NSSize::new(w - 12.0 - pill_w - 8.0 - f.origin.x, f.size.height));
                }
            }
            let pill = pill_button(
                mtm,
                "Reply",
                None,
                &font(12.0, Weight::Semibold),
                &NSColor::whiteColor(),
                &white(0.16),
                12.0,
                &t,
                rect(w - 12.0 - pill_w, top + 22.0, pill_w, 24.0),
            );
            row.addSubview(&pill);
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
        let eng2 = self.engine.clone();
        let rule = n.app.rule_key();
        let app_name = n.app.display_name.clone();
        let entries: Vec<(&str, Action)> = vec![
            ("Open", Box::new(open_fn2)),
            ("Copy", Box::new(move || crate::sharing::copy_text(&text))),
            ("Dismiss", Box::new(move || eng.send(Command::DismissNotification(id)))),
            (
                "Never show this app",
                Box::new(move || {
                    eng2.send(Command::MuteApp(rule.clone()));
                    tracing::info!(app = %app_name, "app muted from the notification row");
                }),
            ),
        ];
        // SAFETY: the menu's targets are kept alive by `row_targets`.
        unsafe { overlay.setMenu(Some(&self.menu(entries))) };
        row.addSubview(&overlay);
        set_accessibility_label(&row, &format!("{}, {}", n.plain_text(), relative_spoken(n.timestamp_ms, now_ms)));
        row
    }

    /// Allow / Deny (agent access) or Cancel (Spotify sign-in), as capsules
    /// across the bottom of an authorization card.
    fn auth_buttons(&self, row: &FlippedView, target: &AuthTarget, w: f64, h: f64) {
        let mtm = self.mtm;
        let f = font(13.0, Weight::Semibold);
        let fg = NSColor::whiteColor();
        let quiet = white(0.16);
        let y = h - 12.0 - 28.0;
        let add = |title: &str, x: f64, bw: f64, primary: bool, strong: bool| {
            let t = target.clone();
            let handler = self.row_targets.add(mtm, move |_| {
                with_controller(|c| c.answer_auth(&t, primary));
            });
            let bg = if strong { rgba(0.039, 0.518, 1.0, 1.0) } else { quiet.clone() };
            let b = pill_button(mtm, title, None, &f, &fg, &bg, 14.0, &handler, rect(x, y, bw, 28.0));
            row.addSubview(&b);
        };
        match target {
            AuthTarget::Agent(_) | AuthTarget::ToolPermission(_) => {
                let bw = (w - 24.0 - 8.0) / 2.0;
                add("Deny", 12.0, bw, false, false);
                add("Allow", 12.0 + bw + 8.0, bw, true, true);
            }
            AuthTarget::Spotify => add("Cancel", 12.0, w - 24.0, true, false),
        }
    }

    fn render_tray(&self, snap: &Snapshot) {
        let p = &self.palette;
        let mtm = self.mtm;
        remove_all_subviews(&self.tray_doc);
        *self.tray_paths.borrow_mut() = snap.tray.iter().map(|t| t.path.clone()).collect();
        let f = font(12.0, Weight::Semibold);
        let count = if snap.tray.is_empty() { String::new() } else { format!("  {}", snap.tray.len()) };
        self.tray_header
            .setAttributedStringValue(&attributed_parts(&[("Tray", &f, &p.primary), (&count, &f, &p.tertiary)]));
        self.tray_empty.setHidden(!snap.tray.is_empty());
        self.tray_scroll.setHidden(snap.tray.is_empty());
        let rows = snap.tray.len().div_ceil(3);
        let visible_h = self.tray_scroll.frame().size.height;
        self.tray_doc.setFrame(rect(0.0, 0.0, self.tray_width, (rows as f64 * (TILE_H + 4.0)).max(visible_h)));
        let cw = (self.tray_width - 2.0 * 4.0) / 3.0;
        for (i, item) in snap.tray.iter().enumerate() {
            let (c, r) = (i % 3, i / 3);
            // Shelved clipboard entries: click to copy back to the pasteboard.
            if let Some(text) = &item.text {
                let is_link = notchforge_core::tray::Tray::is_link(item);
                let text = text.clone();
                let t = self._targets.add(mtm, move |_| {
                    crate::sharing::copy_text(&text);
                });
                let tile = symbol_button(
                    mtm,
                    if is_link { "link" } else { "doc.on.doc" },
                    18.0,
                    "Click to copy",
                    &p.primary,
                    &t,
                    rect(c as f64 * (cw + 4.0), r as f64 * (TILE_H + 4.0), cw, TILE_H),
                );
                make_card(&tile, &white(0.08), 8.0);
                let name =
                    label(mtm, &item.name, &font(10.5, Weight::Regular), &p.primary, rect(4.0, 49.0, cw - 8.0, 14.0));
                name.setAlignment(NSTextAlignment::Center);
                name.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
                tile.addSubview(&name);
                let pin_id = item.id;
                let pinned = item.pinned;
                let pt = self._targets.add(mtm, move |_| {
                    crate::app::send(notchforge_core::engine::Command::TrayTogglePin(pin_id));
                });
                let pin = symbol_button(
                    mtm,
                    if pinned { "pin.fill" } else { "pin" },
                    9.0,
                    if pinned { "Unpin" } else { "Keep forever" },
                    &p.secondary,
                    &pt,
                    rect(cw - 16.0, 2.0, 14.0, 14.0),
                );
                tile.addSubview(&pin);
                self.tray_doc.addSubview(&tile);
                continue;
            }
            let tile = TrayItemView::create(
                mtm,
                rect(c as f64 * (cw + 4.0), r as f64 * (TILE_H + 4.0), cw, TILE_H),
                item.path.clone(),
            );
            let is_image = item.path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "heic" | "gif" | "tiff" | "webp")
            });
            let thumb = if is_image { crate::icons::file_image(&item.path) } else { None };
            let icon = match &thumb {
                // Images show themselves, in a white frame (as on the board).
                Some(img) => {
                    let v = NSImageView::imageViewWithImage(img, mtm);
                    v.setImageScaling(objc2_app_kit::NSImageScaling::ScaleAxesIndependently);
                    v.setFrame(rect((cw - ICON) / 2.0, 0.0, ICON, ICON));
                    v.setWantsLayer(true);
                    if let Some(l) = v.layer() {
                        l.setCornerRadius(5.0);
                        l.setMasksToBounds(true);
                        l.setBorderWidth(1.5);
                        l.setBorderColor(Some(&objc2_app_kit::NSColor::whiteColor().CGColor()));
                    }
                    v
                }
                None => {
                    let v = NSImageView::imageViewWithImage(&crate::icons::file_icon(&item.path), mtm);
                    v.setImageScaling(objc2_app_kit::NSImageScaling::ScaleProportionallyUpOrDown);
                    v.setFrame(rect((cw - ICON) / 2.0, 0.0, ICON, ICON));
                    v
                }
            };
            let is_pdf = item.path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
            if item.is_dir {
                // The board's folder: a back tab and body in blue with a lighter front.
                let x0 = (cw - ICON) / 2.0;
                for (r, color, rad) in [
                    (rect(x0 + 3.0, 8.0, 18.0, 8.0), (0.227, 0.608, 0.937), 3.0),
                    (rect(x0 + 3.0, 12.0, 40.0, 27.0), (0.227, 0.608, 0.937), 4.0),
                    (rect(x0 + 3.0, 18.0, 40.0, 21.0), (0.361, 0.722, 1.0), 4.0),
                ] {
                    tile.addSubview(&shape_view(mtm, r, &rgba(color.0, color.1, color.2, 1.0), rad));
                }
            } else if is_pdf {
                // The board's PDF tile: a white page with a red badge.
                let x0 = (cw - 38.0) / 2.0;
                let page = shape_view(mtm, rect(x0, 0.0, 38.0, ICON), &rgba(0.949, 0.949, 0.969, 1.0), 4.0);
                let badge = shape_view(mtm, rect(5.0, 29.0, 28.0, 11.0), &rgba(0.851, 0.176, 0.125, 1.0), 2.5);
                let f = font(7.5, Weight::Semibold);
                let txt = label(mtm, "PDF", &f, &rgba(1.0, 1.0, 1.0, 1.0), rect(0.0, 0.0, 28.0, 11.0));
                txt.setAlignment(NSTextAlignment::Center);
                badge.addSubview(&txt);
                page.addSubview(&badge);
                tile.addSubview(&page);
            } else {
                tile.addSubview(&icon);
            }

            let name = label(mtm, &item.name, &font(11.0, Weight::Regular), &p.primary, rect(0.0, 49.0, cw, 14.0));
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
            let (p5, p6) = (item.path.clone(), item.path.clone());
            let entries: Vec<(&str, Action)> = vec![
                (
                    "Open",
                    Box::new(move || {
                        crate::sharing::open_path(&p1);
                    }),
                ),
                (
                    "Quick Look",
                    Box::new(move || {
                        let _ = std::process::Command::new("/usr/bin/qlmanage").arg("-p").arg(&p5).spawn();
                    }),
                ),
                (
                    "Copy Text",
                    Box::new(move || {
                        // Screenshots often carry text: OCR them via the
                        // Shortcuts action if the user made one, else nothing.
                        let _ = std::process::Command::new("/usr/bin/shortcuts")
                            .args(["run", "Copy Text from Screenshot"])
                            .arg(&p6)
                            .spawn();
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
    /// Show the reply bar for a message: who it's from and what it says.
    pub fn show_reply_bar(&self, n: &Notification) {
        self.reply_label.setAttributedStringValue(&attributed(
            &format!("Reply to {} — {}", n.app.display_name, n.title),
            &font(12.0, Weight::Semibold),
            &self.palette.primary,
        ));
        crate::ui::set_text(&self.reply_field, "");
        self.reply_bar.setHidden(false);
        // Keep the bar above the notifications list.
        self.root.addSubview(&self.reply_bar);
    }

    /// Hide the reply bar and drop the typed text.
    pub fn hide_reply_bar(&self) {
        self.reply_bar.setHidden(true);
        crate::ui::set_text(&self.reply_field, "");
        self.reply_field.abortEditing();
    }

    /// The reply text field, for the controller to focus.
    pub fn reply_field(&self) -> Retained<NSTextField> {
        self.reply_field.clone()
    }

    /// What the user typed.
    pub fn reply_text(&self) -> String {
        self.reply_field.stringValue().to_string()
    }

    /// The calendar card: connected accounts, then the next events with their
    /// calendar colours. Click an event to open it in Calendar.
    fn render_calendar(&self, snap: &Snapshot) {
        let mtm = self.mtm;
        let p = &self.palette;
        let cal = &snap.calendar;
        let w = self.calendar_card.frame().size.width;
        if self.calendar_last.borrow().as_ref() == Some(cal) {
            return;
        }
        *self.calendar_last.borrow_mut() = Some(cal.clone());
        self.calendar_targets.clear();
        remove_all_subviews(&self.calendar_doc);

        // One row: the next event, else the first reminder due. Without either,
        // the weather's hourly forecast shows there instead.
        let now = notchforge_core::time::now_ms();
        let shows = cal.granted && (!cal.events.is_empty() || !cal.reminders.is_empty());
        self.calendar_card.setHidden(!shows);
        if !shows {
            return;
        }
        self.calendar_doc.addSubview(&shape_view(mtm, rect(12.0, 0.0, w - 24.0, 0.5), &white(0.3), 0.0));
        let y = 12.0;
        let (title, color, action): (String, (u8, u8, u8), Option<Labelled>) = match cal.events.first() {
            Some(ev) => {
                let soon = now + 15 * 60 * 1000 >= ev.starts_at_ms && now <= ev.ends_at_ms;
                let join = ev.meeting_url.clone().filter(|_| soon).map(|url| {
                    let open: Box<dyn Fn()> = Box::new(move || {
                        crate::sharing::open_url(&url);
                    });
                    ("Join", open)
                });
                (format!("{} {}", relative(ev.starts_at_ms, now), ev.title), ev.color.unwrap_or((10, 132, 255)), join)
            }
            None => {
                let rem = &cal.reminders[0];
                let id = rem.id.clone();
                let done: Box<dyn Fn()> = Box::new(move || {
                    crate::app::with_controller(|c| c.complete_plan(&id));
                });
                (rem.title.clone(), (255, 159, 10), Some(("Done", done)))
            }
        };
        let (r, g, b) = color;
        self.calendar_doc.addSubview(&shape_view(
            mtm,
            rect(12.0, y + 4.0, 7.0, 7.0),
            &rgba(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, 1.0),
            3.5,
        ));
        let button_w = if action.is_some() { 46.0 } else { 0.0 };
        self.calendar_doc.addSubview(&label(
            mtm,
            &title,
            &font(12.0, Weight::Medium),
            &p.primary,
            rect(24.0, y, w - 36.0 - button_w, 15.0),
        ));
        if let Some((text, f)) = action {
            let t = self.calendar_targets.add(mtm, move |_| f());
            self.calendar_doc.addSubview(&pill_button(
                mtm,
                text,
                None,
                &font(11.0, Weight::Semibold),
                &NSColor::whiteColor(),
                &white(0.2),
                10.0,
                &t,
                rect(w - 12.0 - 42.0, y - 3.0, 42.0, 20.0),
            ));
        }
    }

    pub fn refresh_controls(&self, display_id: u32) {
        let cfg = self.config.borrow().clone();
        let b = if cfg.controls.brightness { brightness::get() } else { None };
        self.brightness.setHidden(b.is_none());
        if let Some(v) = b {
            self.brightness.set_value(v as f64);
        }
        let v = if cfg.controls.volume { volume::get() } else { None };
        self.volume.setHidden(v.is_none());
        if let Some(v) = v {
            let muted = volume::muted().unwrap_or(false);
            self.volume.set_value(if muted { 0.0 } else { v as f64 });
        }
        self.set_mic_muted(mic::muted());

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
                &ink(0.46),
                16.0,
            );
            self.resolution.setHidden(info.modes.is_empty());
            *self.display.borrow_mut() = Some(info);
        }
        self.layout_status();
    }

    /// Reflect the microphone's mute state in the toggle button (`None` hides
    /// it: no input device, or one without a mute control).
    pub fn set_mic_muted(&self, muted: Option<bool>) {
        let Some(muted) = muted else {
            self.mic_button.setHidden(true);
            return;
        };
        self.mic_button.setHidden(false);
        set_button_symbol(&self.mic_button, if muted { "mic.slash.fill" } else { "mic.fill" }, 13.0, "Microphone mute");
        self.layout_status();
    }

    /// Show fixed control values instead of reading the hardware (self-test
    /// renders of the design boards).
    pub fn preview_controls(&self, brightness: f64, volume: f64, resolution: &str) {
        self.brightness.setHidden(false);
        self.brightness.set_value(brightness);
        self.volume.setHidden(false);
        self.volume.set_value(volume);
        style_pill(
            &self.resolution,
            resolution,
            Some("display"),
            &font(12.0, Weight::Medium),
            &self.palette.primary,
            &ink(0.46),
            16.0,
        );
        self.resolution.setHidden(false);
        self.layout_status();
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

#[cfg(test)]
mod tab_tests {
    use super::Tab;

    #[test]
    fn arrows_wrap_around_the_tab_bar() {
        assert_eq!(Tab::Overview.step(1), Tab::Planner);
        assert_eq!(Tab::Overview.step(-1), Tab::Agents);
        assert_eq!(Tab::Agents.step(1), Tab::Overview);
        assert_eq!(Tab::nth(3), Some(Tab::Media));
        assert_eq!(Tab::nth(0), None);
        assert_eq!(Tab::nth(6), None);
    }
}
