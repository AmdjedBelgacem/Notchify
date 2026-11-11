//! **Agents**: every coding session, the one waiting for you first, beside
//! your plan's limits. After the *3 · Open: Agents* board:
//!
//! ```text
//! Sessions                                   Claude limits
//! ╭ ● Claude Code              12 min ╮      ╭ ◔ 62%  Session · 16:00 ╮
//! │   Notchify · waiting for you      │      │ ◔ 31%  Week · Mon      │
//! │ ┌ Edit crates/…/mod.rs ─────────┐ │      │ ───────────────────────│
//! │               ( Deny )( Allow )   │      │ Tokens today   1.84 M  │
//! ╰───────────────────────────────────╯      ╰────────────────────────╯
//! ╭ ● Gemini CLI  site · reading     2 min ╮        ( Usage details )
//! ```

use super::kit::{self, Btn, GAP, PAD};
use super::now::{asks, open_notification};
use crate::app::with_controller;
use crate::ui::{FlippedView, Targets, Weight, font, label, rect, remove_all_subviews};
use notchforge_core::engine::{Command, Snapshot};
use notchforge_core::model::{AuthTarget, Notification, RunningSession, UsageAgent};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSScrollView, NSTextAlignment};
use std::cell::Cell;

/// The limits column.
const SIDE_W: f64 = 208.0;

pub struct AgentsPage {
    pub root: Retained<FlippedView>,
    mtm: MainThreadMarker,
    scroll: Retained<NSScrollView>,
    doc: Retained<FlippedView>,
    side: Retained<FlippedView>,
    targets: Targets,
    last: Cell<(u64, i64)>,
}

/// "1.84 M", "238 K", "912".
pub fn tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.0} K", n as f64 / 1_000.0),
        _ => format!("{:.2} M", n as f64 / 1_000_000.0),
    }
}

/// Does notification `n` come from (and wait on) `agent`'s session?
fn from_agent(n: &Notification, agent: UsageAgent) -> bool {
    let first = agent.display_name().split_whitespace().next().unwrap_or("");
    asks(n) && n.app.display_name.starts_with(first)
}

impl AgentsPage {
    pub fn new(mtm: MainThreadMarker, w: f64, h: f64, top: f64) -> Self {
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        let col_w = w - 2.0 * PAD - SIDE_W - GAP;
        let y0 = top + 6.0;
        root.addSubview(&kit::heading(mtm, "Sessions", rect(PAD + 4.0, y0, col_w, 16.0)));
        let doc = FlippedView::new(mtm, rect(0.0, 0.0, col_w, 10.0));
        let scroll = kit::scroller(mtm, rect(PAD, y0 + 22.0, col_w, h - y0 - 22.0 - 12.0), &doc);
        root.addSubview(&scroll);
        let side = FlippedView::new(mtm, rect(w - PAD - SIDE_W, y0, SIDE_W, h - y0 - 12.0));
        root.addSubview(&side);
        Self { root, mtm, scroll, doc, side, targets: Targets::default(), last: Cell::new((u64::MAX, 0)) }
    }

    pub fn update(&self, snap: &Snapshot) {
        let key = (snap.generation, snap.now_ms / 15_000);
        if self.last.replace(key) == key {
            return;
        }
        remove_all_subviews(&self.doc);
        remove_all_subviews(&self.side);
        self.targets.clear();
        self.sessions(snap);
        self.limits(snap);
    }

    fn sessions(&self, snap: &Snapshot) {
        let w = self.scroll.frame().size.width;
        let mut asked: Vec<u64> = Vec::new();
        let mut list: Vec<(&RunningSession, Option<&Notification>)> = snap
            .usage
            .running
            .iter()
            .map(|s| {
                let q = snap.notifications.iter().find(|n| from_agent(n, s.agent) && !asked.contains(&n.id.0));
                if let Some(q) = q {
                    asked.push(q.id.0);
                }
                (s, q)
            })
            .collect();
        list.sort_by_key(|(s, q)| (q.is_none(), !s.working, std::cmp::Reverse(s.last_ms)));
        let mut y = 0.0;
        for (s, q) in &list {
            y = self.session(s, *q, snap.now_ms, y, w) + 8.0;
        }
        if list.is_empty() {
            let h = self.scroll.frame().size.height;
            let a = label(
                self.mtm,
                "No agents running",
                &font(14.0, Weight::Semibold),
                &kit::text(),
                rect(0.0, h / 2.0 - 30.0, w, 19.0),
            );
            a.setAlignment(NSTextAlignment::Center);
            self.doc.addSubview(&a);
            let b = label(
                self.mtm,
                "Sessions show here as soon as an agent starts.",
                &font(13.0, Weight::Regular),
                &kit::text2(),
                rect(0.0, h / 2.0 - 6.0, w, 18.0),
            );
            b.setAlignment(NSTextAlignment::Center);
            self.doc.addSubview(&b);
            y = h;
        }
        let visible = self.scroll.frame().size.height;
        self.doc.setFrame(rect(0.0, 0.0, w, y.max(visible)));
    }

    /// One session: state dot, name, where it is and what it's doing, and
    /// the question it waits on with its answers.
    fn session(&self, s: &RunningSession, q: Option<&Notification>, now: i64, y: f64, w: f64) -> f64 {
        let mtm = self.mtm;
        let inset = 12.0;
        let iw = w - 2.0 * inset;
        let command =
            q.is_some_and(|q| matches!(q.auth_target(), Some(AuthTarget::ToolPermission(_) | AuthTarget::Agent(_))));
        let h = inset + 40.0 + if q.is_some() { (if command { 42.0 } else { 0.0 }) + 8.0 + 30.0 } else { 0.0 } + inset;
        let card = kit::tile_view(mtm, rect(0.0, y, w, h));
        let (color, state): (Retained<NSColor>, String) = match (q, s.working) {
            (Some(_), _) => (kit::amber(), "waiting for you".into()),
            (None, true) => (kit::accent(), "working".into()),
            (None, false) => (kit::green(), "finished".into()),
        };
        card.addSubview(&kit::dot(mtm, inset, inset + 6.0, 8.0, &color));
        card.addSubview(&kit::line(
            mtm,
            s.agent.display_name(),
            14.0,
            Weight::Semibold,
            &kit::text(),
            rect(inset + 16.0, inset, iw - 16.0 - 70.0, 19.0),
        ));
        let detail = match &s.model {
            m if !m.is_empty() => format!("{} · {state} · {m}", s.project),
            _ => format!("{} · {state}", s.project),
        };
        card.addSubview(&kit::line(
            mtm,
            &detail,
            12.0,
            Weight::Regular,
            &kit::text2(),
            rect(inset + 16.0, inset + 21.0, iw - 16.0, 16.0),
        ));
        let since = if s.working { s.turn_started_ms.unwrap_or(s.started_ms) } else { s.last_ms };
        let t = label(
            mtm,
            &kit::span(now - since),
            &crate::ui::mono_digits(11.5, Weight::Regular),
            &kit::text2(),
            rect(w - inset - 70.0, inset + 2.0, 70.0, 15.0),
        );
        t.setAlignment(NSTextAlignment::Right);
        card.addSubview(&t);
        if let Some(q) = q {
            let mut cy = inset + 44.0;
            if command {
                let what = q.subtitle.clone().filter(|x| !x.is_empty()).unwrap_or_else(|| q.title.clone());
                card.addSubview(&kit::mono_box(mtm, &what, rect(inset, cy, iw, 34.0)));
                cy += 42.0;
            }
            let mut x = w - inset;
            let mut add = |title: &str, kind: Btn, f: Box<dyn Fn()>| {
                let bw = kit::button_width(title, 28.0);
                x -= bw;
                card.addSubview(&kit::button(mtm, &self.targets, title, kind, rect(x, cy, bw, 28.0), f));
                x -= 8.0;
            };
            match q.auth_target() {
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
                _ => {
                    let (n2, id) = (q.clone(), q.id);
                    add("Open", Btn::Primary, Box::new(move || open_notification(&n2)));
                    add(
                        "Dismiss",
                        Btn::Secondary,
                        Box::new(move || crate::app::send(Command::DismissNotification(id))),
                    );
                }
            }
            if !command {
                // The question itself, left of its answers.
                card.addSubview(&kit::line(
                    mtm,
                    &q.title,
                    13.0,
                    Weight::Regular,
                    &kit::text(),
                    rect(inset, cy + 5.0, (x - inset - 8.0).max(40.0), 18.0),
                ));
            }
        }
        self.doc.addSubview(&card);
        y + h
    }

    /// The first connected agent's limits as rings, then today's tokens.
    fn limits(&self, snap: &Snapshot) {
        let mtm = self.mtm;
        let w = SIDE_W;
        let lim = snap.usage.limits.iter().find(|l| l.connected && !l.gauges.is_empty());
        let title = lim.map_or_else(
            || "Limits".to_owned(),
            |l| format!("{} limits", l.agent.display_name().split_whitespace().next().unwrap_or("")),
        );
        self.side.addSubview(&kit::heading(mtm, &title, rect(4.0, 0.0, w - 8.0, 16.0)));
        let gauges: Vec<_> =
            lim.map(|l| l.gauges.iter().filter(|g| g.label != "Context").take(2).collect()).unwrap_or_default();
        let rows = gauges.len() as f64;
        let today = snap.usage.ranges.first().map(|r| r.counts.main());
        let h = 14.0
            + rows * 66.0
            + if today.is_some() { 1.0 + 42.0 } else { 0.0 }
            + if rows == 0.0 && today.is_none() { 60.0 } else { 0.0 };
        let card = kit::tile_view(mtm, rect(0.0, 22.0, w, h));
        let mut y = 14.0;
        for g in gauges {
            let fill = match g.percent {
                95.. => kit::red(),
                80..=94 => rgba_amber(),
                _ => kit::accent(),
            };
            card.addSubview(&super::expanded::ring_view(
                mtm,
                rect(14.0, y, 52.0, 52.0),
                g.percent as f64 / 100.0,
                &fill,
            ));
            card.addSubview(&label(
                mtm,
                &format!("{}%", g.percent),
                &crate::ui::mono_digits(18.0, Weight::Semibold),
                &kit::text(),
                rect(78.0, y + 6.0, w - 90.0, 22.0),
            ));
            let reset = g
                .resets_at_ms
                .map(|r| format!(" · resets {}", super::usage::reset_label(r, snap.now_ms)))
                .unwrap_or_default();
            card.addSubview(&kit::line(
                mtm,
                &format!("{}{reset}", g.label),
                12.0,
                Weight::Regular,
                &kit::text2(),
                rect(78.0, y + 30.0, w - 90.0, 16.0),
            ));
            y += 66.0;
        }
        if let Some(t) = today {
            if rows > 0.0 {
                card.addSubview(&kit::rule(mtm, 14.0, y, w - 28.0));
                y += 1.0;
            }
            card.addSubview(&label(
                mtm,
                "Tokens today",
                &font(12.0, Weight::Regular),
                &kit::text2(),
                rect(14.0, y + 13.0, 100.0, 16.0),
            ));
            let v = label(
                mtm,
                &tokens(t),
                &crate::ui::mono_digits(14.0, Weight::Semibold),
                &kit::text(),
                rect(w - 14.0 - 90.0, y + 12.0, 90.0, 18.0),
            );
            v.setAlignment(NSTextAlignment::Right);
            card.addSubview(&v);
        } else if rows == 0.0 {
            card.addSubview(&kit::line(
                mtm,
                "Connect an agent to see its limits.",
                12.0,
                Weight::Regular,
                &kit::text2(),
                rect(14.0, 22.0, w - 28.0, 16.0),
            ));
        }
        self.side.addSubview(&card);
        let bw = kit::button_width("Usage details", 28.0);
        self.side.addSubview(&kit::button(
            mtm,
            &self.targets,
            "Usage details",
            Btn::Secondary,
            rect(w - bw, 22.0 + h + 10.0, bw, 28.0),
            || {
                with_controller(|c| c.show_expanded_tab(super::expanded::Tab::Agents));
            },
        ));
    }
}

fn rgba_amber() -> Retained<NSColor> {
    crate::ui::rgba(1.0, 0.624, 0.039, 1.0)
}

#[cfg(test)]
mod tests {
    use super::tokens;

    #[test]
    fn token_counts_read_short() {
        assert_eq!(tokens(912), "912");
        assert_eq!(tokens(238_400), "238 K");
        assert_eq!(tokens(1_840_000), "1.84 M");
    }
}
