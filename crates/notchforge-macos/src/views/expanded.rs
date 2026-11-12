//! The open island, after the *Notchisland redesign* boards: a tab row just
//! below the notch, and one space under it at a time.
//!
//! ```text
//!                 ┌───[ notch ]───┐            only the neck crosses the menu bar
//! ╭───────────────╯               ╰──────────────────────────────────────────╮
//! │ (Now) Music  Agents  Day  Shelf                      ☾   🔕   ⚙          │
//! │                                                                          │
//! │   the space: Now, Music, Agents, Day or Shelf (or Usage, from Agents)    │
//! ╰──────────────────────────────────────────────────────────────────────────╯
//! ```
//!
//! Each space is its own view ([`super::now`], [`super::media`],
//! [`super::agents`], [`super::day`]); Shelf lives here because its file tiles
//! share the tray's drag-out view and context menu.

use super::media::MediaPage;
use super::usage::UsagePage;
use crate::app::with_controller;
use crate::controls::mic;
use crate::panel::TrayItemView;
use crate::ui::{
    FlippedView, Palette, Targets, Weight, attributed, font, label, make_card, mono_digits, pill_button, rect,
    remove_all_subviews, rgba, set_accessibility_label, shape_view, symbol_button, symbol_view,
};
use notchforge_core::config::Config;
use notchforge_core::engine::{Command, EngineHandle, Snapshot};
use notchforge_core::geometry::{LayoutKind, NotchLayout};
use notchforge_core::model::Notification;
use notchforge_core::tray::format_bytes;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSButton, NSColor, NSImageView, NSMenu, NSMenuItem, NSScrollView, NSTextAlignment, NSTextField};
use objc2_foundation::NSRect;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::Arc;

const PAD: f64 = 14.0;
const TILE_H: f64 = 64.0;
const TILE_R: f64 = 16.0;
const ICON: f64 = 46.0;

type Action = Box<dyn Fn()>;

/// Pages of the expanded panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Now: what needs you, what's playing, what's next.
    Overview,
    /// Day: weather, today, reminders, timer.
    Planner,
    /// Music.
    Media,
    /// Agents: sessions and limits.
    Live,
    /// Usage details, opened from Agents.
    Agents,
    /// Shelf: files and clipboard.
    Shelf,
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
/// In the tab row's order: Now, Music, Agents, Day, Shelf (⌘1–5). Usage
/// details open from Agents.
pub const TABS: [Tab; 5] = [Tab::Overview, Tab::Media, Tab::Live, Tab::Planner, Tab::Shelf];

/// Each tab's name in the tab row, and the width of its pill.
fn tab_title(tab: Tab) -> (&'static str, f64) {
    match tab {
        Tab::Overview => ("Now", 52.0),
        Tab::Media => ("Music", 62.0),
        Tab::Live => ("Agents", 68.0),
        Tab::Planner => ("Day", 50.0),
        Tab::Agents => ("Usage", 62.0),
        Tab::Shelf => ("Shelf", 56.0),
    }
}

pub struct ExpandedView {
    pub root: Retained<FlippedView>,
    /// The five spaces, one shown at a time, and Usage details.
    now: super::now::NowPage,
    pub media: MediaPage,
    agents: super::agents::AgentsPage,
    day: super::day::DayPage,
    pub usage: UsagePage,
    /// Shelf: dropped files and shelved clipboard text.
    shelf: Retained<FlippedView>,
    shelf_doc: Retained<FlippedView>,
    shelf_scroll: Retained<NSScrollView>,
    shelf_highlight: Cell<bool>,
    shelf_targets: Targets,
    tray_paths: RefCell<Vec<PathBuf>>,

    tab: Cell<Tab>,
    tab_buttons: Vec<Retained<NSButton>>,
    /// A call in progress, at the right of the tab row.
    call_pill: Retained<FlippedView>,
    call_pill_text: Retained<NSTextField>,
    /// The panel is expanded (drives the music page's ticker).
    visible: Cell<bool>,

    /// The quick-reply bar: an overlay strip under the tab row.
    reply_bar: Retained<FlippedView>,
    reply_label: Retained<NSTextField>,
    reply_field: Retained<NSTextField>,
    /// Keeps the Send pill's action target alive.
    _reply_targets: Targets,

    mtm: MainThreadMarker,
    palette: Palette,
    engine: EngineHandle,
    config: RefCell<Arc<Config>>,
    /// Keeps the tab row's action targets alive.
    _targets: Targets,
    /// Targets of the shelf's context menus, rebuilt with each render.
    row_targets: Targets,
    last_rendered: Cell<u64>,
}

/// For actions whose result only matters to the log.
fn drop_bool(_: bool) {}

fn white(a: f64) -> Retained<NSColor> {
    rgba(1.0, 1.0, 1.0, a)
}

/// The one surface on the black island: a flat, slightly lifted tile.
pub fn tile_color() -> Retained<NSColor> {
    rgba(0.086, 0.086, 0.094, 1.0)
}

/// One widget tile on the black island: flat, no outline.
pub fn glass_tile(mtm: MainThreadMarker, frame: NSRect) -> Retained<FlippedView> {
    shape_view(mtm, frame, &tile_color(), TILE_R)
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
        // The tab row sits just below the notch; every space starts below it.
        // Nothing is drawn in the menu bar beside the notch (the neck).
        let notch_h = layout.notch.rect.height;
        let nh = notch_h + notchforge_core::geometry::TAB_BAR;
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        let targets = Targets::default();
        let ty = notch_h + (notchforge_core::geometry::TAB_BAR - 24.0) / 2.0 + 2.0;

        // ── the spaces ──
        let now = super::now::NowPage::new(mtm, w, h, nh);
        root.addSubview(&now.root);
        let media = MediaPage::new(mtm, w, h, nh + 8.0, p);
        media.root.setHidden(true);
        root.addSubview(&media.root);
        let agents = super::agents::AgentsPage::new(mtm, w, h, nh);
        agents.root.setHidden(true);
        root.addSubview(&agents.root);
        let day = super::day::DayPage::new(mtm, w, h, nh);
        day.root.setHidden(true);
        root.addSubview(&day.root);
        let usage = UsagePage::new(mtm, w, h, nh, p);
        usage.root.setHidden(true);
        root.addSubview(&usage.root);
        let shelf = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        shelf.setHidden(true);
        let shelf_doc = FlippedView::new(mtm, rect(0.0, 0.0, w - 2.0 * PAD, 10.0));
        let shelf_scroll =
            super::kit::scroller(mtm, rect(PAD, nh + 6.0, w - 2.0 * PAD, h - nh - 6.0 - 12.0), &shelf_doc);
        shelf.addSubview(&shelf_scroll);
        root.addSubview(&shelf);

        // ── the tab row: spaces as text pills; Focus, pause and Settings at the end ──
        let mut tab_x = PAD;
        let tab_buttons: Vec<Retained<NSButton>> = TABS
            .iter()
            .map(|tab| {
                let tab = *tab;
                let (title, base) = tab_title(tab);
                // At least the designed width; wider when a translation needs it.
                let tw = base.max(
                    crate::ui::text_width(&attributed(title, &font(13.0, Weight::Medium), &p.secondary)).ceil() + 24.0,
                );
                let t = targets.add(mtm, move |_| {
                    with_controller(|c| c.show_expanded_tab(tab));
                });
                let b = pill_button(
                    mtm,
                    title,
                    None,
                    &font(13.0, Weight::Medium),
                    &p.secondary,
                    &white(0.0),
                    14.0,
                    &t,
                    rect(tab_x, ty - 2.0, tw, 28.0),
                );
                tab_x += tw + 4.0;
                root.addSubview(&b);
                b
            })
            .collect();
        let row_button = |sym: &str, tip: &str, slot: f64, action: fn()| {
            let t = targets.add(mtm, move |_| action());
            let b = symbol_button(
                mtm,
                sym,
                13.0,
                tip,
                &p.secondary,
                &t,
                rect(w - 18.0 - 28.0 - slot * 34.0, ty - 2.0, 28.0, 28.0),
            );
            make_card(&b, &white(0.0), 14.0);
            root.addSubview(&b);
            b
        };
        // As on the boards: the tab keys, then Settings. Focus and pausing
        // notifications are on Day, beside the microphone.
        row_button("gearshape", "Settings", 0.0, crate::app::show_settings);
        // Quick capture: screenshots, screen recording, voice memos.
        row_button("camera.viewfinder", "Capture", 2.55, crate::capture::show_menu);
        let keys = label(
            mtm,
            "⌘1–5",
            &mono_digits(11.0, Weight::Regular),
            &super::kit::text3(),
            rect(w - 18.0 - 28.0 - 8.0 - 44.0, ty + 5.0, 44.0, 14.0),
        );
        keys.setAlignment(NSTextAlignment::Right);
        root.addSubview(&keys);

        // A call in progress: its clock and caller, and hang up.
        let call_w = 200.0;
        let call_pill = FlippedView::new(mtm, rect(w - 18.0 - 28.0 - 60.0 - 8.0 - call_w, ty, call_w, 24.0));
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

        // ── quick reply bar: an overlay under the tab row ──
        let reply_bar = FlippedView::new(mtm, rect(PAD, nh + 6.0, w - 2.0 * PAD, 48.0));
        make_card(&reply_bar, &super::kit::raised(), 14.0);
        let reply_label = label(mtm, "", &font(12.0, Weight::Semibold), &p.primary, rect(14.0, 6.0, 240.0, 16.0));
        reply_bar.addSubview(&reply_label);
        let reply_targets = Targets::default();
        let rt = reply_targets.add(mtm, move |_| {
            crate::app::with_controller(|c| c.send_reply());
        });
        let reply_field =
            NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(14.0, 26.0, w - 2.0 * PAD - 100.0, 18.0));
        reply_field.setEditable(true);
        reply_field.setBordered(false);
        reply_field.setBezeled(false);
        reply_field.setDrawsBackground(false);
        reply_field.setFocusRingType(objc2_app_kit::NSFocusRingType::None);
        reply_field.setTextColor(Some(&p.primary));
        reply_field.setFont(Some(&font(13.0, Weight::Regular)));
        reply_field.setPlaceholderString(Some(&crate::ui::ns("Type your reply, ⏎ to send")));
        // SAFETY: a valid target and selector; the target outlives the field.
        unsafe {
            reply_field.setTarget(Some(rt.as_any()));
            reply_field.setAction(Some(crate::ui::ActionTarget::action()));
        }
        reply_bar.addSubview(&reply_field);
        let send = pill_button(
            mtm,
            "Send",
            None,
            &font(12.5, Weight::Semibold),
            &super::kit::on_accent(),
            &super::kit::accent(),
            15.0,
            &rt,
            rect(w - 2.0 * PAD - 80.0, 9.0, 66.0, 30.0),
        );
        reply_bar.addSubview(&send);
        reply_bar.setHidden(true);
        root.addSubview(&reply_bar);

        Self {
            root,
            now,
            media,
            agents,
            day,
            usage,
            shelf,
            shelf_doc,
            shelf_scroll,
            shelf_highlight: Cell::new(false),
            shelf_targets: Targets::default(),
            tray_paths: RefCell::new(Vec::new()),
            tab: Cell::new(Tab::Overview),
            tab_buttons,
            call_pill,
            call_pill_text,
            visible: Cell::new(false),
            reply_bar,
            reply_label,
            reply_field,
            _reply_targets: reply_targets,
            mtm,
            palette,
            engine,
            config: RefCell::new(config),
            _targets: targets,
            row_targets: Targets::default(),
            last_rendered: Cell::new(u64::MAX),
        }
    }

    pub fn tab(&self) -> Tab {
        self.tab.get()
    }

    /// Switch pages. Opening the music page asks the providers for fresh data.
    /// A clock tick: Day redraws its running timer or stopwatch if it shows.
    pub fn tick_day(&self, snap: &Snapshot) {
        if self.tab.get() == Tab::Planner {
            self.day.update(snap);
        }
    }

    pub fn show_tab(&self, tab: Tab) {
        let previous = self.tab.replace(tab);
        self.now.root.setHidden(tab != Tab::Overview);
        self.media.root.setHidden(tab != Tab::Media);
        self.agents.root.setHidden(tab != Tab::Live);
        self.day.root.setHidden(tab != Tab::Planner);
        self.shelf.setHidden(tab != Tab::Shelf);
        self.usage.root.setHidden(tab != Tab::Agents);
        for (b, t) in self.tab_buttons.iter().zip(TABS) {
            let on = t == tab || (t == Tab::Live && tab == Tab::Agents);
            let (title, _) = tab_title(t);
            let (fg, weight) =
                if on { (&self.palette.primary, Weight::Semibold) } else { (&self.palette.secondary, Weight::Medium) };
            b.setAttributedTitle(&attributed(title, &font(13.0, weight), fg));
            // The current space: a lighter pill; the rest sit on black.
            make_card(b, &white(if on { 0.17 } else { 0.0 }), 14.0);
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
        // Files on their way in: show where they'll land.
        if on != self.shelf_highlight.replace(on) {
            if on {
                self.show_tab(Tab::Shelf);
            }
            if let Some(snap) = crate::app::with_controller(|c| c.current_snapshot()) {
                self.render_shelf(&snap);
            }
        }
    }

    /// Re-render dynamic content. Skips work if this snapshot was rendered.
    pub fn update(&self, snap: &Snapshot) {
        if self.last_rendered.replace(snap.generation) == snap.generation {
            return;
        }
        self.row_targets.clear();
        *self.tray_paths.borrow_mut() = snap.tray.iter().map(|t| t.path.clone()).collect();
        self.now.update(snap, self.config.borrow().notifications.quick_reply);
        self.media.update(snap);
        self.usage.update(&snap.usage, self.config.borrow().agents.token_usage);
        self.day.update(snap);
        self.agents.update(snap);
        self.render_shelf(snap);
    }

    /// Read what the panel shows from the system; called as it opens.
    pub fn refresh_controls(&self, _display_id: u32) {
        self.day.set_mic_muted(mic::muted());
    }

    /// The microphone's mute state, for Day's switch (`None`: no input that
    /// can be muted).
    pub fn set_mic_muted(&self, muted: Option<bool>) {
        self.day.set_mic_muted(muted);
    }

    /// Show the call in progress, or that there isn't one. Called every second.
    pub fn set_call(&self, call: Option<&notchforge_core::calls::ActiveCall>, now_ms: i64) {
        if let Some(c) = call {
            let clock = notchforge_core::calls::call_clock(c.started_ms, now_ms);
            crate::ui::set_text(&self.call_pill_text, &format!("{clock} · {}", c.caller));
        }
        self.call_pill.setHidden(call.is_none());
    }

    /// The Shelf space: a drop area holding the files, actions for them all,
    /// then clipboard snippets to copy back.
    fn render_shelf(&self, snap: &Snapshot) {
        use super::kit::{self, Btn};
        let mtm = self.mtm;
        remove_all_subviews(&self.shelf_doc);
        self.shelf_targets.clear();
        let w = self.shelf_scroll.frame().size.width - super::kit::GUTTER;
        let files: Vec<_> = snap.tray.iter().filter(|t| t.text.is_none()).collect();
        let clips: Vec<_> = snap.tray.iter().filter(|t| t.text.is_some()).collect();
        let lit = self.shelf_highlight.get();

        // ── files, in a drop area ──
        let (cell, gap) = (92.0, 8.0);
        let per_row = (((w - 24.0 + gap) / (cell + gap)).floor() as usize).max(1);
        let rows = files.len().div_ceil(per_row).max(1) as f64;
        let box_h = if files.is_empty() { 96.0 } else { 24.0 + rows * (TILE_H + 8.0) };
        let drop = kit::tile_view(mtm, rect(0.0, 0.0, w, box_h));
        if let Some(l) = drop.layer() {
            l.setBorderWidth(if lit { 1.5 } else { 0.0 });
            l.setBorderColor(Some(&kit::accent().CGColor()));
            if lit {
                l.setBackgroundColor(Some(&rgba(0.392, 0.824, 1.0, 0.08).CGColor()));
            }
        }
        if files.is_empty() {
            let a = label(
                mtm,
                if lit { "Drop to add" } else { "Drop files on the notch" },
                &font(14.0, Weight::Semibold),
                &*if lit { kit::accent() } else { kit::text() },
                rect(0.0, 30.0, w, 19.0),
            );
            a.setAlignment(NSTextAlignment::Center);
            drop.addSubview(&a);
            let b = label(
                mtm,
                "They wait here; drag them back out to move them anywhere",
                &font(12.0, Weight::Regular),
                &kit::text2(),
                rect(0.0, 52.0, w, 16.0),
            );
            b.setAlignment(NSTextAlignment::Center);
            drop.addSubview(&b);
        }
        for (i, item) in files.iter().enumerate() {
            let (c, r) = ((i % per_row) as f64, (i / per_row) as f64);
            let tile = self.file_tile(item, rect(12.0 + c * (cell + gap), 12.0 + r * (TILE_H + 8.0), cell, TILE_H));
            drop.addSubview(&tile);
        }
        self.shelf_doc.addSubview(&drop);
        let mut y = box_h + 10.0;
        if !files.is_empty() {
            let paths: Vec<PathBuf> = files.iter().map(|t| t.path.clone()).collect();
            let (p1, p2) = (paths.clone(), paths);
            let mut x = 0.0;
            let mut add = |title: &str, f: Box<dyn Fn()>| {
                let bw = kit::button_width(title, 28.0);
                self.shelf_doc.addSubview(&kit::button(
                    mtm,
                    &self.shelf_targets,
                    title,
                    Btn::Secondary,
                    rect(x, y, bw, 28.0),
                    f,
                ));
                x += bw + 8.0;
            };
            add("AirDrop all", Box::new(move || drop_bool(crate::sharing::airdrop(&p1))));
            add("Show in Finder", Box::new(move || crate::sharing::reveal(&p2)));
            let eng = self.engine.clone();
            add("Clear shelf", Box::new(move || eng.send(Command::TrayClear)));
            y += 28.0 + 18.0;
        }

        // ── clipboard ──
        self.shelf_doc.addSubview(&kit::heading(mtm, "Clipboard", rect(4.0, y, 120.0, 16.0)));
        let hint = label(
            mtm,
            "⌃⌥C shelves the clipboard",
            &font(11.5, Weight::Regular),
            &kit::text3(),
            rect(w - 204.0, y, 200.0, 16.0),
        );
        hint.setAlignment(NSTextAlignment::Right);
        self.shelf_doc.addSubview(&hint);
        y += 22.0;
        if clips.is_empty() {
            self.shelf_doc.addSubview(&kit::line(
                mtm,
                "Nothing shelved yet",
                13.0,
                Weight::Regular,
                &kit::text2(),
                rect(4.0, y + 4.0, w - 8.0, 18.0),
            ));
            y += 28.0;
        }
        for item in clips {
            let text = item.text.clone().unwrap_or_default();
            let row = kit::tile_view(mtm, rect(0.0, y, w, 40.0));
            if let Some(l) = row.layer() {
                l.setCornerRadius(12.0);
            }
            let link = notchforge_core::tray::Tray::is_link(item);
            let shown = text.replace('\n', " ");
            let l = if link {
                kit::line(mtm, &shown, 12.5, Weight::Regular, &kit::text(), rect(12.0, 11.0, w - 12.0 - 130.0, 18.0))
            } else {
                kit::line(mtm, &shown, 13.0, Weight::Regular, &kit::text(), rect(12.0, 11.0, w - 12.0 - 130.0, 18.0))
            };
            if link {
                l.setFont(Some(&crate::ui::mono_digits(12.5, Weight::Regular)));
            }
            row.addSubview(&l);
            let id = item.id;
            let eng = self.engine.clone();
            row.addSubview(&kit::round_button(
                mtm,
                &self.shelf_targets,
                "xmark",
                "Remove",
                false,
                rect(w - 38.0, 7.0, 26.0, 26.0),
                move || {
                    eng.send(Command::TrayRemove(id));
                },
            ));
            let bw = kit::button_width("Copy", 26.0);
            row.addSubview(&kit::button(
                mtm,
                &self.shelf_targets,
                "Copy",
                Btn::Secondary,
                rect(w - 44.0 - bw, 7.0, bw, 26.0),
                move || {
                    crate::sharing::copy_text(&text);
                },
            ));
            self.shelf_doc.addSubview(&row);
            y += 48.0;
        }
        let visible = self.shelf_scroll.frame().size.height;
        self.shelf_doc.setFrame(rect(0.0, 0.0, w, y.max(visible)));
    }

    /// One file on the shelf: its icon (images show themselves), its name,
    /// drag it out, double-click to open, right-click for the rest.
    fn file_tile(&self, item: &notchforge_core::model::TrayItem, frame: NSRect) -> Retained<TrayItemView> {
        let mtm = self.mtm;
        let p = &self.palette;
        let cw = frame.size.width;
        let tile = TrayItemView::create(mtm, frame, item.path.clone());
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
        let size =
            item.bytes.map(format_bytes).unwrap_or_else(|| if item.is_dir { "Folder".into() } else { String::new() });
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
                    // Built-in Vision OCR first; the Shortcuts action is
                    // the fallback for languages Vision's default model
                    // handles poorly.
                    let text = crate::vision::recognize_text(&p6);
                    match text {
                        Some(text) => crate::sharing::copy_text(&text),
                        None => {
                            let _ = std::process::Command::new("/usr/bin/shortcuts")
                                .args(["run", "Copy Text from Screenshot"])
                                .arg(&p6)
                                .spawn();
                        }
                    }
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
        tile
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
                    &crate::ui::ns(&crate::i18n::tr(title)),
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
        // Keep the bar above the space under it.
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
}

#[cfg(test)]
mod tab_tests {
    use super::Tab;

    #[test]
    fn arrows_wrap_around_the_tab_bar() {
        // Now, Music, Agents, Day, Shelf.
        assert_eq!(Tab::Overview.step(1), Tab::Media);
        assert_eq!(Tab::Overview.step(-1), Tab::Shelf);
        assert_eq!(Tab::Shelf.step(1), Tab::Overview);
        assert_eq!(Tab::nth(2), Some(Tab::Media));
        assert_eq!(Tab::nth(4), Some(Tab::Planner));
        assert_eq!(Tab::nth(5), Some(Tab::Shelf));
        assert_eq!(Tab::nth(0), None);
        assert_eq!(Tab::nth(6), None);
    }
}
