//! **Now**: the space the island opens on. One column, most urgent first,
//! after the *3 · Open: Now* board:
//!
//! ```text
//! ╭ ● Needs you  Claude Code · Notchify                 now ╮
//! │ Wants to run a command                                   │
//! │ ┌ cargo test -p notchforge-core ──────────────────────┐ │
//! │                                        ( Deny )( Allow ) │
//! ╰──────────────────────────────────────────────────────────╯
//!  ▣ Slow Light  Harbor Lines · Spotify                 (❚❚)
//!    1:24 ━━━━━━━━━──────────────────────────────── 3:52
//!  ─────────────────────────────────────────────────────────
//!  ▣ Design review  15:00 · in 28 min                 ( Join )
//!  ─────────────────────────────────────────────────────────
//!  ▣ Gemini CLI  site · working for 2 min
//!  Earlier
//!  ◉ Messages  Léa: Dinner at 8?                         12 min
//! ```
//!
//! Every row is a whole target: the playing row opens Music, an agent opens
//! Agents, a notification opens where it came from.

use super::kit::{self, Btn, GAP, PAD};
use crate::app::with_controller;
use crate::ui::{FlippedView, Targets, Weight, click_overlay, font, label, rect, remove_all_subviews, shape_view};
use notchforge_core::engine::{Command, Snapshot};
use notchforge_core::model::{AuthTarget, Notification, NotificationKind};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSScrollView;
use std::cell::Cell;

/// Questions shown at the top; more wait on the Agents space.
const MAX_ASKS: usize = 2;
/// Working agents listed.
const MAX_RUNNING: usize = 2;
/// Earlier notifications listed under the rest.
const MAX_EARLIER: usize = 8;
const ROW_H: f64 = 56.0;
const ART: f64 = 44.0;

pub struct NowPage {
    pub root: Retained<FlippedView>,
    mtm: MainThreadMarker,
    scroll: Retained<NSScrollView>,
    doc: Retained<FlippedView>,
    targets: Targets,
    last: Cell<(u64, i64)>,
}

/// Is `n` waiting on an answer from the user?
pub fn asks(n: &Notification) -> bool {
    n.auth_target().is_some() || n.kind == NotificationKind::NeedsInput
}

/// Open where a notification came from: its link, else its app.
pub fn open_notification(n: &Notification) {
    let opened = n.url.as_deref().is_some_and(crate::sharing::open_url);
    if !opened {
        if let Some(b) = n.app.bundle_id.as_deref() {
            crate::sharing::open_app(b);
        }
    }
}

/// Weekday initials and day of month for a date tile: ("WED", 11).
pub fn date_parts(ms: i64) -> (&'static str, i32) {
    const DAYS: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
    // SAFETY: localtime_r fills a caller-owned struct.
    let tm = unsafe {
        let t = (ms / 1000) as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    };
    (DAYS[tm.tm_wday.clamp(0, 6) as usize], tm.tm_mday)
}

impl NowPage {
    pub fn new(mtm: MainThreadMarker, w: f64, h: f64, top: f64) -> Self {
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        let doc = FlippedView::new(mtm, rect(0.0, 0.0, w - 2.0 * PAD, 10.0));
        let scroll = kit::scroller(mtm, rect(PAD, top + 6.0, w - 2.0 * PAD, h - top - 6.0 - 12.0), &doc);
        root.addSubview(&scroll);
        Self { root, mtm, scroll, doc, targets: Targets::default(), last: Cell::new((u64::MAX, 0)) }
    }

    /// `quick_reply`: messages can be answered from the notch.
    pub fn update(&self, snap: &Snapshot, quick_reply: bool) {
        // Redraw on new state, and every few seconds for times and the timeline.
        let key = (snap.generation, snap.now_ms / 5_000);
        if self.last.replace(key) == key {
            return;
        }
        remove_all_subviews(&self.doc);
        self.targets.clear();
        let w = self.scroll.frame().size.width;
        let mut y = 0.0;

        let questions: Vec<&Notification> = snap.notifications.iter().filter(|n| asks(n)).take(MAX_ASKS).collect();
        for n in &questions {
            y = self.ask_card(n, snap.now_ms, y, w) + GAP;
        }
        let mut rows = 0;
        let mut row = |page: &Self, y: &mut f64, f: &dyn Fn(&Self, f64, f64) -> f64| {
            if rows > 0 {
                page.doc.addSubview(&kit::rule(page.mtm, 4.0, *y + 4.0, w - 8.0));
                *y += 9.0;
            }
            *y = f(page, *y, w);
            rows += 1;
        };
        if snap.media.primary().is_some() {
            row(self, &mut y, &|p, y, w| p.playing(snap, y, w));
        }
        if let Some(e) = next_event(snap) {
            row(self, &mut y, &move |p, y, w| p.next_event(e, snap.now_ms, y, w));
        }
        for s in snap.usage.running.iter().filter(|s| s.working).take(MAX_RUNNING) {
            row(self, &mut y, &move |p, y, w| p.running(s, snap.now_ms, y, w));
        }
        let earlier: Vec<&Notification> = snap.notifications.iter().filter(|n| !asks(n)).take(MAX_EARLIER).collect();
        if !earlier.is_empty() {
            y += if y > 0.0 { 14.0 } else { 0.0 };
            self.doc.addSubview(&kit::heading(self.mtm, "Earlier", rect(4.0, y, w - 8.0, 16.0)));
            let cw = kit::button_width("Clear", 22.0);
            self.doc.addSubview(&kit::button(
                self.mtm,
                &self.targets,
                "Clear",
                Btn::Secondary,
                rect(w - cw - 4.0, y - 3.0, cw, 22.0),
                || {
                    crate::app::send(Command::ClearNotifications);
                },
            ));
            y += 24.0;
            for n in earlier {
                y = self.earlier(n, snap.now_ms, y, w, quick_reply);
            }
        }
        if y == 0.0 {
            y = self.quiet(snap, w);
        }
        let visible = self.scroll.frame().size.height;
        self.doc.setFrame(rect(0.0, 0.0, w, y.max(visible)));
    }

    /// A question from an agent (or a sign-in), with its answers.
    fn ask_card(&self, n: &Notification, now: i64, y: f64, w: f64) -> f64 {
        let mtm = self.mtm;
        let inset = 14.0;
        let detail =
            n.subtitle.clone().filter(|s| !s.is_empty()).or_else(|| Some(n.body.clone()).filter(|b| !b.is_empty()));
        // A tool request shows its command in mono; anything else, a plain line.
        let command = matches!(n.auth_target(), Some(AuthTarget::ToolPermission(_) | AuthTarget::Agent(_)));
        let detail_h = match (&detail, command) {
            (None, _) => 0.0,
            (Some(_), true) => 10.0 + 34.0,
            (Some(_), false) => 2.0 + 18.0,
        };
        let h = inset + 18.0 + 6.0 + 20.0 + detail_h + 12.0 + kit::BUTTON_H + inset;
        let card = kit::tile_view(mtm, rect(0.0, y, w, h));
        let iw = w - 2.0 * inset;
        card.addSubview(&kit::dot(mtm, inset, inset + 5.0, 8.0, &kit::amber()));
        card.addSubview(&label(
            mtm,
            "Needs you",
            &font(12.0, Weight::Semibold),
            &kit::amber(),
            rect(inset + 14.0, inset, 80.0, 16.0),
        ));
        card.addSubview(&kit::line(
            mtm,
            &n.app.display_name,
            12.0,
            Weight::Regular,
            &kit::text2(),
            rect(inset + 92.0, inset, iw - 92.0 - 60.0, 16.0),
        ));
        let when = label(
            mtm,
            &kit::ago(now, n.timestamp_ms),
            &font(12.0, Weight::Regular),
            &kit::text3(),
            rect(w - inset - 60.0, inset, 60.0, 16.0),
        );
        when.setAlignment(objc2_app_kit::NSTextAlignment::Right);
        card.addSubview(&when);
        let mut cy = inset + 24.0;
        card.addSubview(&kit::line(mtm, &n.title, 14.0, Weight::Medium, &kit::text(), rect(inset, cy, iw, 19.0)));
        cy += 20.0;
        match detail {
            Some(d) if command => {
                cy += 10.0;
                card.addSubview(&kit::mono_box(mtm, &d, rect(inset, cy, iw, 34.0)));
                cy += 34.0;
            }
            Some(d) => {
                cy += 2.0;
                card.addSubview(&kit::line(mtm, &d, 13.0, Weight::Regular, &kit::text2(), rect(inset, cy, iw, 18.0)));
                cy += 18.0;
            }
            None => {}
        }
        cy += 12.0;
        let mut x = w - inset;
        let mut add = |title: &str, kind: Btn, f: Box<dyn Fn()>| {
            let bw = kit::button_width(title, kit::BUTTON_H);
            x -= bw;
            card.addSubview(&kit::button(mtm, &self.targets, title, kind, rect(x, cy, bw, kit::BUTTON_H), f));
            x -= 8.0;
        };
        match n.auth_target() {
            Some(t @ (AuthTarget::Agent(_) | AuthTarget::ToolPermission(_))) => {
                let t2 = t.clone();
                add(
                    "Allow",
                    Btn::Primary,
                    Box::new(move || {
                        with_controller(|c| c.answer_auth(&t, true));
                    }),
                );
                add(
                    "Deny",
                    Btn::Secondary,
                    Box::new(move || {
                        with_controller(|c| c.answer_auth(&t2, false));
                    }),
                );
            }
            Some(t @ AuthTarget::Spotify) => {
                add(
                    "Cancel",
                    Btn::Secondary,
                    Box::new(move || {
                        with_controller(|c| c.answer_auth(&t, true));
                    }),
                );
            }
            None => {
                let (n2, id) = (n.clone(), n.id);
                add("Open", Btn::Primary, Box::new(move || open_notification(&n2)));
                add("Dismiss", Btn::Secondary, Box::new(move || crate::app::send(Command::DismissNotification(id))));
            }
        }
        self.doc.addSubview(&card);
        y + h
    }

    /// What's playing, with its timeline and play / pause. Opens Music.
    fn playing(&self, snap: &Snapshot, y: f64, w: f64) -> f64 {
        let mtm = self.mtm;
        let Some(n) = snap.media.primary() else { return y };
        let open = self.targets.add(mtm, |_| {
            with_controller(|c| c.show_expanded_tab(super::expanded::Tab::Media));
        });
        self.doc.addSubview(&click_overlay(mtm, &open, rect(0.0, y, w - 52.0, ROW_H), "Open Music"));
        let ay = y + (ROW_H - ART) / 2.0;
        let art = n.artwork_path.as_deref().and_then(crate::icons::file_image);
        match art {
            Some(img) => {
                if let Some(v) = crate::ui::image_box(mtm, rect(4.0, ay, ART, ART), &img) {
                    if let Some(l) = v.layer() {
                        l.setCornerRadius(10.0);
                        l.setMasksToBounds(true);
                    }
                    self.doc.addSubview(&v);
                }
            }
            None => {
                let tile = kit::tile_view(mtm, rect(4.0, ay, ART, ART));
                tile.addSubview(&crate::ui::symbol_view(
                    mtm,
                    "music.note",
                    17.0,
                    &kit::text2(),
                    rect(12.0, 12.0, 20.0, 20.0),
                    "",
                ));
                self.doc.addSubview(&tile);
            }
        }
        let x = 4.0 + ART + 12.0;
        let tw = w - x - 52.0;
        let by = if n.artist.is_empty() {
            n.player.display_name().to_owned()
        } else {
            format!("{} · {}", n.artist, n.player.display_name())
        };
        let title = crate::ui::rich_label(
            mtm,
            &crate::ui::attributed_parts(&[
                (n.title.as_str(), &*font(14.0, Weight::Semibold), &*kit::text()),
                ("  ", &*font(13.0, Weight::Regular), &*kit::text2()),
                (&by, &*font(13.0, Weight::Regular), &*kit::text2()),
            ]),
            rect(x, ay + 2.0, tw, 19.0),
        );
        title.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        self.doc.addSubview(&title);
        if let (Some(dur), Some(pos)) = (n.duration, n.position) {
            let elapsed = if n.is_playing() { (snap.now_ms - n.sampled_at_ms).max(0) as f64 / 1000.0 } else { 0.0 };
            let pos = (pos + elapsed).min(dur);
            let f = crate::ui::mono_digits(11.0, Weight::Regular);
            let ty = ay + 26.0;
            self.doc.addSubview(&label(mtm, &super::media::clock(pos), &f, &kit::text2(), rect(x, ty, 40.0, 14.0)));
            let end = label(mtm, &super::media::clock(dur), &f, &kit::text2(), rect(x + tw - 40.0, ty, 40.0, 14.0));
            end.setAlignment(objc2_app_kit::NSTextAlignment::Right);
            self.doc.addSubview(&end);
            let frac = if dur > 0.0 { pos / dur } else { 0.0 };
            self.doc.addSubview(&kit::bar(mtm, rect(x + 40.0, ty + 5.0, tw - 80.0, 4.0), frac, &kit::text()));
        }
        let player = n.player;
        let playing = n.is_playing();
        self.doc.addSubview(&kit::round_button(
            mtm,
            &self.targets,
            if playing { "pause.fill" } else { "play.fill" },
            if playing { "Pause" } else { "Play" },
            true,
            rect(w - 40.0, y + (ROW_H - 36.0) / 2.0, 36.0, 36.0),
            move || crate::media::control(player, notchforge_core::model::MediaAction::PlayPause),
        ));
        y + ROW_H
    }

    /// The next meeting or event today, with Join when it has a link.
    fn next_event(&self, e: &notchforge_core::model::CalendarEvent, now: i64, y: f64, w: f64) -> f64 {
        let mtm = self.mtm;
        let ay = y + (ROW_H - ART) / 2.0;
        let (wd, day) = date_parts(e.starts_at_ms);
        let tile = kit::tile_view(mtm, rect(4.0, ay, ART, ART));
        let top = label(mtm, wd, &font(9.5, Weight::Bold), &kit::red(), rect(0.0, 6.0, ART, 12.0));
        top.setAlignment(objc2_app_kit::NSTextAlignment::Center);
        tile.addSubview(&top);
        let num = label(mtm, &day.to_string(), &font(17.0, Weight::Semibold), &kit::text(), rect(0.0, 18.0, ART, 21.0));
        num.setAlignment(objc2_app_kit::NSTextAlignment::Center);
        tile.addSubview(&num);
        self.doc.addSubview(&tile);
        let x = 4.0 + ART + 12.0;
        let join_w = if e.meeting_url.is_some() { kit::button_width("Join", 30.0) + 8.0 } else { 0.0 };
        let tw = w - x - join_w - 4.0;
        self.doc.addSubview(&kit::line(
            mtm,
            &e.title,
            14.0,
            Weight::Semibold,
            &kit::text(),
            rect(x, ay + 4.0, tw, 19.0),
        ));
        let when = if e.starts_at_ms <= now {
            format!("Until {} · now", kit::clock(e.ends_at_ms))
        } else {
            format!("{} · {}", kit::clock(e.starts_at_ms), kit::until(now, e.starts_at_ms))
        };
        let when = match &e.location {
            Some(l) if !l.is_empty() => format!("{when} · {l}"),
            _ => when,
        };
        self.doc.addSubview(&kit::line(mtm, &when, 13.0, Weight::Regular, &kit::text2(), rect(x, ay + 24.0, tw, 17.0)));
        if let Some(url) = e.meeting_url.clone() {
            let bw = join_w - 8.0;
            self.doc.addSubview(&kit::button(
                mtm,
                &self.targets,
                "Join",
                Btn::Secondary,
                rect(w - bw - 4.0, y + (ROW_H - 30.0) / 2.0, bw, 30.0),
                move || {
                    crate::sharing::open_url(&url);
                },
            ));
        }
        y + ROW_H
    }

    /// An agent at work. Opens Agents.
    fn running(&self, s: &notchforge_core::model::RunningSession, now: i64, y: f64, w: f64) -> f64 {
        let mtm = self.mtm;
        let open = self.targets.add(mtm, |_| {
            with_controller(|c| c.show_expanded_tab(super::expanded::Tab::Live));
        });
        self.doc.addSubview(&click_overlay(mtm, &open, rect(0.0, y, w, ROW_H), "Open Agents"));
        let ay = y + (ROW_H - ART) / 2.0;
        let tile = kit::tile_view(mtm, rect(4.0, ay, ART, ART));
        tile.addSubview(&crate::ui::symbol_view(
            mtm,
            "apple.terminal",
            17.0,
            &kit::text(),
            rect(11.0, 12.0, 22.0, 20.0),
            "",
        ));
        self.doc.addSubview(&tile);
        let x = 4.0 + ART + 12.0;
        let tw = w - x - 4.0;
        let since = s.turn_started_ms.unwrap_or(s.started_ms);
        let line = crate::ui::attributed_parts(&[
            (s.agent.display_name(), &font(14.0, Weight::Semibold), &kit::text()),
            (
                &format!("  {} · working for {}", s.project, kit::span(now - since)),
                &font(13.0, Weight::Regular),
                &kit::text2(),
            ),
        ]);
        let l = crate::ui::rich_label(mtm, &line, rect(x, ay + 4.0, tw, 19.0));
        l.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        self.doc.addSubview(&l);
        // A short sweep that moves with the clock: busy, without a fake percentage.
        let track = shape_view(mtm, rect(x, ay + 32.0, tw, 3.0), &kit::hairline(), 1.5);
        let phase = ((now / 500) % 20) as f64 / 20.0;
        track.addSubview(&shape_view(mtm, rect((tw * 0.7) * phase, 0.0, tw * 0.3, 3.0), &kit::accent(), 1.5));
        self.doc.addSubview(&track);
        y + ROW_H
    }

    /// An earlier notification, one line. Click opens it.
    fn earlier(&self, n: &Notification, now: i64, y: f64, w: f64, quick_reply: bool) -> f64 {
        let mtm = self.mtm;
        let h = 44.0;
        let n2 = n.clone();
        let open = self.targets.add(mtm, move |_| open_notification(&n2));
        self.doc.addSubview(&click_overlay(mtm, &open, rect(0.0, y, w, h), &format!("Open {}", n.title)));
        let icon = super::app_icon_view(mtm, rect(4.0, y + 8.0, 28.0, 28.0));
        super::set_app_icon(&icon, &n.app);
        self.doc.addSubview(&icon);
        let x = 44.0;
        // Messages can be answered right here.
        let replyable = quick_reply
            && n.kind == NotificationKind::Message
            && n.app.category() == notchforge_core::apps::AppCategory::Messaging;
        let reply_w = if replyable { kit::button_width("Reply", 26.0) + 8.0 } else { 0.0 };
        let tw = w - x - 64.0 - reply_w;
        let what =
            if n.body.is_empty() { n.title.clone() } else { format!("{}: {}", n.title, n.body.replace('\n', " ")) };
        let line = crate::ui::attributed_parts(&[
            (&n.app.display_name, &*font(13.0, Weight::Semibold), &*kit::text()),
            ("  ", &*font(13.0, Weight::Regular), &*kit::text2()),
            (&what, &*font(13.0, Weight::Regular), &*kit::text2()),
        ]);
        let l = crate::ui::rich_label(mtm, &line, rect(x, y + 13.0, tw, 18.0));
        l.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        self.doc.addSubview(&l);
        let when = label(
            mtm,
            &kit::ago(now, n.timestamp_ms),
            &font(12.0, Weight::Regular),
            &kit::text3(),
            rect(w - 64.0, y + 14.0, 60.0, 16.0),
        );
        when.setAlignment(objc2_app_kit::NSTextAlignment::Right);
        self.doc.addSubview(&when);
        if replyable {
            let id = n.id;
            let bw = reply_w - 8.0;
            self.doc.addSubview(&kit::button(
                mtm,
                &self.targets,
                "Reply",
                Btn::Secondary,
                rect(w - 64.0 - bw - 4.0, y + 9.0, bw, 26.0),
                move || {
                    with_controller(|c| c.start_reply(id));
                },
            ));
        }
        y + h
    }

    /// Nothing waiting, playing or next: say so, quietly.
    fn quiet(&self, snap: &Snapshot, w: f64) -> f64 {
        let mtm = self.mtm;
        let h = self.scroll.frame().size.height;
        let clock = label(
            mtm,
            &kit::clock(snap.now_ms),
            &crate::ui::mono_digits(40.0, Weight::Semibold),
            &kit::text(),
            rect(0.0, h / 2.0 - 52.0, w, 48.0),
        );
        clock.setAlignment(objc2_app_kit::NSTextAlignment::Center);
        self.doc.addSubview(&clock);
        let sub = label(
            mtm,
            "Nothing needs you",
            &font(14.0, Weight::Regular),
            &kit::text2(),
            rect(0.0, h / 2.0 + 2.0, w, 19.0),
        );
        sub.setAlignment(objc2_app_kit::NSTextAlignment::Center);
        self.doc.addSubview(&sub);
        h
    }
}

/// The next event that hasn't ended, today, not all-day.
fn next_event(snap: &Snapshot) -> Option<&notchforge_core::model::CalendarEvent> {
    let horizon = snap.now_ms + 12 * 3_600_000;
    let pool = if snap.calendar.today.is_empty() { &snap.calendar.events } else { &snap.calendar.today };
    pool.iter()
        .filter(|e| !e.all_day && e.ends_at_ms > snap.now_ms && e.starts_at_ms < horizon)
        .min_by_key(|e| e.starts_at_ms)
}
