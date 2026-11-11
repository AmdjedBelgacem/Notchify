//! **Day**: the weather, what's on today, your reminders and a timer, with
//! batteries and the microphone along the bottom. After the *3 · Open: Day*
//! board:
//!
//! ```text
//! ╭ 14°            ☁ ╮  Today
//! │ Partly cloudy    │  ● Design review   15:00–15:45 · in 28 min  ( Join )
//! │ 15  16  17  18   │  ● 1:1 with Sam    16:30–17:00
//! ╰──────────────────╯  ● Gym             19:00–20:00
//! Reminders              Timer
//! ○ Send the release     ╭ 18:42  Focus · 25 min     ( Stop ) ╮
//! ○ Renew domain  today  (1m)(5m)(10m)(25m)(45m)
//! ───────────────────────────────────────────────────────────────
//! Mac 82% · AirPods 70%                               ( Mute mic )
//! ```

use super::kit::{self, Btn, PAD};
use crate::app::with_controller;
use crate::ui::{FlippedView, Targets, Weight, font, label, rect, remove_all_subviews};
use notchforge_core::engine::{Command, Snapshot};
use notchforge_core::model::CalendarEvent;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSTextAlignment;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const WEATHER_W: f64 = 236.0;
const TOP_H: f64 = 136.0;
const TIMER_W: f64 = 272.0;
const PRESETS: [(i64, &str); 5] = [(1, "1m"), (5, "5m"), (10, "10m"), (25, "25m"), (45, "45m")];

pub struct DayPage {
    pub root: Retained<FlippedView>,
    mtm: MainThreadMarker,
    body: Retained<FlippedView>,
    targets: Targets,
    last: Cell<(u64, i64)>,
    /// Reminders ticked off here, hidden before the calendar catches up.
    done: Rc<RefCell<Vec<String>>>,
    mic_muted: Cell<Option<bool>>,
}

impl DayPage {
    pub fn new(mtm: MainThreadMarker, w: f64, h: f64, top: f64) -> Self {
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        let body = FlippedView::new(mtm, rect(PAD, top + 6.0, w - 2.0 * PAD, h - top - 6.0 - 12.0));
        root.addSubview(&body);
        Self {
            root,
            mtm,
            body,
            targets: Targets::default(),
            last: Cell::new((u64::MAX, 0)),
            done: Rc::default(),
            mic_muted: Cell::new(None),
        }
    }

    pub fn set_mic_muted(&self, muted: Option<bool>) {
        self.mic_muted.set(muted);
        self.last.set((u64::MAX, 0));
    }

    pub fn update(&self, snap: &Snapshot) {
        // The timer counts down by the second; otherwise every half minute.
        let tick = if snap.timer.is_some() { snap.now_ms / 1_000 } else { snap.now_ms / 30_000 };
        let key = (snap.generation, tick);
        if self.last.replace(key) == key {
            return;
        }
        remove_all_subviews(&self.body);
        self.targets.clear();
        let w = self.body.frame().size.width;
        let h = self.body.frame().size.height;
        self.weather(snap, rect(0.0, 0.0, WEATHER_W, TOP_H));
        self.today(snap, WEATHER_W + PAD, w - WEATHER_W - PAD);
        let y2 = TOP_H + 14.0;
        let foot = h - 30.0;
        self.reminders(snap, 0.0, y2, w - TIMER_W - PAD, foot - y2 - 8.0);
        self.timer(snap, w - TIMER_W, y2);
        self.footer(snap, foot, w);
    }

    fn weather(&self, snap: &Snapshot, frame: objc2_foundation::NSRect) {
        let mtm = self.mtm;
        let t = kit::tile_view(mtm, frame);
        let w = frame.size.width;
        match &snap.weather {
            Some(wx) => {
                t.addSubview(&label(
                    mtm,
                    &super::temperature(wx.temperature),
                    &font(34.0, Weight::Semibold),
                    &kit::text(),
                    rect(14.0, 10.0, 120.0, 40.0),
                ));
                t.addSubview(&crate::ui::symbol_view(
                    mtm,
                    wx.condition.symbol(wx.is_day),
                    24.0,
                    &kit::text(),
                    rect(w - 14.0 - 30.0, 14.0, 30.0, 28.0),
                    wx.condition.label(),
                ));
                let line = format!(
                    "{} · H {} L {}",
                    wx.condition.label(),
                    super::temperature(wx.high),
                    super::temperature(wx.low)
                );
                t.addSubview(&kit::line(
                    mtm,
                    &line,
                    12.0,
                    Weight::Regular,
                    &kit::text2(),
                    rect(14.0, 52.0, w - 28.0, 16.0),
                ));
                let hours: Vec<_> = wx.hourly.iter().take(5).collect();
                let cw = (w - 28.0) / 5.0;
                for (i, hr) in hours.iter().enumerate() {
                    let x = 14.0 + i as f64 * cw;
                    let a = label(mtm, &hr.hour, &font(11.0, Weight::Regular), &kit::text3(), rect(x, 82.0, cw, 14.0));
                    a.setAlignment(NSTextAlignment::Center);
                    t.addSubview(&a);
                    let b = label(
                        mtm,
                        &super::temperature(hr.temperature),
                        &font(13.0, Weight::Semibold),
                        &kit::text(),
                        rect(x, 98.0, cw, 17.0),
                    );
                    b.setAlignment(NSTextAlignment::Center);
                    t.addSubview(&b);
                }
                let rain = wx.hourly.iter().take(6).find(|h| h.precipitation_chance.is_some_and(|p| p >= 50));
                if let Some(r) = rain {
                    t.addSubview(&kit::line(
                        mtm,
                        &format!("Rain likely from {}", r.hour),
                        11.5,
                        Weight::Regular,
                        &kit::accent(),
                        rect(14.0, 118.0, w - 28.0, 15.0),
                    ));
                }
            }
            None => {
                t.addSubview(&kit::line(
                    mtm,
                    "Weather",
                    14.0,
                    Weight::Semibold,
                    &kit::text(),
                    rect(14.0, 14.0, w - 28.0, 19.0),
                ));
                t.addSubview(&kit::line(
                    mtm,
                    "Set a place in Settings",
                    12.0,
                    Weight::Regular,
                    &kit::text2(),
                    rect(14.0, 36.0, w - 28.0, 16.0),
                ));
            }
        }
        self.body.addSubview(&t);
    }

    /// Today's events that haven't ended, soonest first.
    fn today(&self, snap: &Snapshot, x: f64, w: f64) {
        let mtm = self.mtm;
        self.body.addSubview(&kit::heading(mtm, "Today", rect(x + 4.0, 0.0, w - 8.0, 16.0)));
        let mut events: Vec<&CalendarEvent> =
            snap.calendar.today.iter().filter(|e| e.ends_at_ms > snap.now_ms).collect();
        events.sort_by_key(|e| (!e.all_day, e.starts_at_ms));
        if events.is_empty() {
            let msg =
                if snap.calendar.granted { "Nothing else on today" } else { "Allow Calendar access to see today" };
            self.body.addSubview(&kit::line(
                mtm,
                msg,
                13.0,
                Weight::Regular,
                &kit::text2(),
                rect(x + 4.0, 26.0, w - 8.0, 18.0),
            ));
            return;
        }
        let mut y = 22.0;
        for (i, e) in events.into_iter().take(3).enumerate() {
            let row_h = 36.0;
            // The next one sits on a tile, as on the Day board.
            if i == 0 {
                let t = kit::tile_view(mtm, rect(x, y - 5.0, w, row_h + 4.0));
                if let Some(l) = t.layer() {
                    l.setCornerRadius(12.0);
                }
                self.body.addSubview(&t);
            }
            let color = e.color.map_or_else(kit::accent, crate::ui::rgb8);
            self.body.addSubview(&kit::dot(mtm, x + 6.0, y + 7.0, 8.0, &color));
            let soon = e.meeting_url.is_some() && e.starts_at_ms - snap.now_ms < 15 * 60_000;
            let join_w = if soon { kit::button_width("Join", 26.0) } else { 0.0 };
            let tw = w - 22.0 - join_w - 6.0;
            self.body.addSubview(&kit::line(
                mtm,
                &e.title,
                13.0,
                Weight::Semibold,
                &kit::text(),
                rect(x + 22.0, y, tw, 17.0),
            ));
            let when = if e.all_day {
                "All day".to_owned()
            } else if e.starts_at_ms > snap.now_ms {
                format!(
                    "{}–{} · {}",
                    kit::clock(e.starts_at_ms),
                    kit::clock(e.ends_at_ms),
                    kit::until(snap.now_ms, e.starts_at_ms)
                )
            } else {
                format!("Now · until {}", kit::clock(e.ends_at_ms))
            };
            self.body.addSubview(&kit::line(
                mtm,
                &when,
                12.0,
                Weight::Regular,
                &kit::text2(),
                rect(x + 22.0, y + 17.0, tw, 16.0),
            ));
            if let (true, Some(url)) = (soon, e.meeting_url.clone()) {
                self.body.addSubview(&kit::button(
                    mtm,
                    &self.targets,
                    "Join",
                    Btn::Secondary,
                    rect(x + w - join_w, y + 4.0, join_w, 26.0),
                    move || {
                        crate::sharing::open_url(&url);
                    },
                ));
            }
            y += row_h + if i == 0 { 6.0 } else { 0.0 };
        }
    }

    /// Up to three open reminders; the circle ticks one off.
    fn reminders(&self, snap: &Snapshot, x: f64, y: f64, w: f64, h: f64) {
        let mtm = self.mtm;
        self.body.addSubview(&kit::heading(mtm, "Reminders", rect(x + 4.0, y, w - 8.0, 16.0)));
        let done = self.done.borrow().clone();
        let open: Vec<_> = snap.calendar.reminders.iter().filter(|r| !done.contains(&r.id)).collect();
        if open.is_empty() {
            let msg = if snap.calendar.granted { "All done" } else { "Allow Reminders access to see them" };
            self.body.addSubview(&kit::line(
                mtm,
                msg,
                13.0,
                Weight::Regular,
                &kit::text2(),
                rect(x + 4.0, y + 26.0, w - 8.0, 18.0),
            ));
            return;
        }
        let rows = ((h - 22.0) / 30.0).floor().max(1.0) as usize;
        let mut ry = y + 22.0;
        for r in open.into_iter().take(rows) {
            let (id, done) = (r.id.clone(), self.done.clone());
            let check = kit::round_button(
                mtm,
                &self.targets,
                "circle",
                "Mark done",
                false,
                rect(x, ry, 26.0, 26.0),
                move || {
                    done.borrow_mut().push(id.clone());
                    with_controller(|c| c.complete_plan(&id));
                },
            );
            check.setContentTintColor(Some(&kit::text2()));
            self.body.addSubview(&check);
            let due = r.due_at_ms.map(|d| kit::due_label(d, snap.now_ms)).unwrap_or_default();
            let overdue = due.starts_with("Overdue") || due.starts_with("Today");
            let due_w = if due.is_empty() { 0.0 } else { 110.0 };
            self.body.addSubview(&kit::line(
                mtm,
                &r.title,
                13.0,
                Weight::Regular,
                &kit::text(),
                rect(x + 32.0, ry + 4.0, w - 32.0 - due_w, 18.0),
            ));
            if !due.is_empty() {
                let color = if overdue { crate::ui::rgba(1.0, 0.412, 0.38, 1.0) } else { kit::text3() };
                let d =
                    label(mtm, &due, &font(12.0, Weight::Regular), &color, rect(x + w - due_w, ry + 5.0, due_w, 16.0));
                d.setAlignment(NSTextAlignment::Right);
                self.body.addSubview(&d);
            }
            ry += 30.0;
        }
    }

    /// The running timer, or one-tap presets.
    fn timer(&self, snap: &Snapshot, x: f64, y: f64) {
        let mtm = self.mtm;
        let w = TIMER_W;
        self.body.addSubview(&kit::heading(mtm, "Timer", rect(x + 4.0, y, w - 8.0, 16.0)));
        let mut ty = y + 22.0;
        if let Some(t) = &snap.timer {
            let row = kit::tile_view(mtm, rect(x, ty, w, 44.0));
            let left = if t.expired { 0 } else { (t.ends_at_ms - snap.now_ms).max(0) };
            let secs = left / 1000;
            let clock = format!("{:02}:{:02}", secs / 60, secs % 60);
            row.addSubview(&label(
                mtm,
                &clock,
                &crate::ui::mono_digits(20.0, Weight::Medium),
                &kit::text(),
                rect(12.0, 10.0, 80.0, 24.0),
            ));
            let what = if t.label.is_empty() {
                kit::span(t.duration_ms)
            } else {
                format!("{} · {}", t.label, kit::span(t.duration_ms))
            };
            row.addSubview(&kit::line(
                mtm,
                &what,
                12.0,
                Weight::Regular,
                &kit::text2(),
                rect(90.0, 14.0, w - 90.0 - 76.0, 16.0),
            ));
            let bw = kit::button_width("Stop", 28.0);
            row.addSubview(&kit::button(
                mtm,
                &self.targets,
                "Stop",
                Btn::Secondary,
                rect(w - 10.0 - bw, 8.0, bw, 28.0),
                || {
                    crate::app::send(Command::CancelTimer);
                },
            ));
            self.body.addSubview(&row);
            ty += 44.0 + 8.0;
        }
        let gap = 6.0;
        let bw = (w - gap * (PRESETS.len() as f64 - 1.0)) / PRESETS.len() as f64;
        for (i, (mins, title)) in PRESETS.iter().enumerate() {
            let ms = mins * 60_000;
            let b = kit::button(
                mtm,
                &self.targets,
                title,
                Btn::Secondary,
                rect(x + i as f64 * (bw + gap), ty, bw, 30.0),
                move || {
                    crate::app::send(Command::StartTimer { duration_ms: ms, label: String::new() });
                },
            );
            crate::ui::make_card(&b, &kit::tile(), 15.0);
            self.body.addSubview(&b);
        }
    }

    /// Batteries, and the microphone switch.
    fn footer(&self, snap: &Snapshot, y: f64, w: f64) {
        let mtm = self.mtm;
        self.body.addSubview(&kit::rule(mtm, 0.0, y - 4.0, w));
        let mut parts: Vec<String> = Vec::new();
        if let Some(b) = &snap.battery {
            if b.present {
                parts.push(format!("Mac {}%", b.percent));
            }
        }
        for d in snap.devices.iter().take(2) {
            parts.push(format!("{} {}", d.name, super::device_levels(d)));
        }
        let text = if parts.is_empty() { "No batteries to show".to_owned() } else { parts.join("   ") };
        let muted = self.mic_muted.get() == Some(true);
        let title = if muted { "Unmute mic" } else { "Mute mic" };
        let bw = kit::button_width(title, 28.0);
        // Focus and pausing notifications sit with the microphone; each is
        // lit while it's on.
        let focus_on = snap.focus.active == Some(true);
        let focus_t = if focus_on { "Focus on" } else { "Focus" };
        let pause_t = if snap.paused { "Alerts paused" } else { "Pause alerts" };
        let (fw, pw) = (kit::button_width(focus_t, 28.0), kit::button_width(pause_t, 28.0));
        let mut x = w - bw - 8.0 - pw - 8.0 - fw;
        let kind = |on: bool| if on { Btn::Primary } else { Btn::Secondary };
        self.body.addSubview(&kit::button(
            mtm,
            &self.targets,
            focus_t,
            kind(focus_on),
            rect(x, y + 2.0, fw, 28.0),
            || {
                with_controller(|c| c.toggle_focus());
            },
        ));
        x += fw + 8.0;
        self.body.addSubview(&kit::button(
            mtm,
            &self.targets,
            pause_t,
            kind(snap.paused),
            rect(x, y + 2.0, pw, 28.0),
            || {
                with_controller(|c| c.toggle_pause());
            },
        ));
        self.body.addSubview(&kit::line(
            mtm,
            &text,
            12.0,
            Weight::Regular,
            &kit::text2(),
            rect(4.0, y + 8.0, w - bw - 8.0 - pw - 8.0 - fw - 16.0, 16.0),
        ));
        let b = kit::button(
            mtm,
            &self.targets,
            title,
            if muted { Btn::Primary } else { Btn::Secondary },
            rect(w - bw, y + 2.0, bw, 28.0),
            || {
                with_controller(|c| c.toggle_mic());
            },
        );
        self.body.addSubview(&b);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn presets_are_short() {
        assert!(super::PRESETS.iter().all(|(m, t)| t.starts_with(&m.to_string())));
    }
}
