//! "Live sessions" page of the expanded panel, built to the *Pinned · Live
//! sessions* board of the redesign twin: the coding-agent sessions working now,
//! what each is doing, and roughly when its turn will finish.
//!
//! ```text
//! ╭ ● Claude Code · Notchify  Needs you ╮ ╭ ● Codex CLI · api-server  Working ╮
//! │ Allow running cargo test?  Deny Allow│ │ Editing auth.rs        ≈ 2 min left │
//! │ This turn 3m 12s ▓▓▓▓▓▓▓▒▒▒ ~4 min   │ ╰────────────────────────────────────╯
//! │ Last 8 turns ▂▅▁▇▄▃▆▄                │ ╭ ● OpenCode · website       Working ╮
//! │ ◯ 76%  Session 2h 14m · 96K · Opus   │ │ Running npm test       ≈ 6 min left │
//! ╰──────────────────────────────────────╯ ╰────────────────────────────────────╯
//! ```
//!
//! How long is left is an estimate: a typical turn for that session (the median
//! of its recent ones) less what the current turn has run.

use super::expanded::{glass_tile, ring_view};
use crate::app::with_controller;
use crate::ui::{
    FlippedView, Palette, Targets, Weight, attributed_parts, font, label, mono_digits, pill_button, rect,
    remove_all_subviews, rgba, rich_label, set_accessibility_label, shape_view, symbol_view,
};
use notchforge_core::apps::KnownApp;
use notchforge_core::engine::Snapshot;
use notchforge_core::model::{AuthTarget, Notification, NotificationKind, RunningSession, UsageAgent, compact_tokens};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSTextAlignment};
use objc2_foundation::NSRect;

const PAD: f64 = 14.0;
const GAP: f64 = 10.0;
const INSET: f64 = 14.0;
/// Up to this many sessions get a card.
const CARDS: usize = 4;

fn white(a: f64) -> Retained<NSColor> {
    rgba(1.0, 1.0, 1.0, a)
}

fn amber() -> Retained<NSColor> {
    rgba(1.0, 0.624, 0.039, 1.0)
}

fn green() -> Retained<NSColor> {
    rgba(0.188, 0.82, 0.345, 1.0)
}

fn agent_color(a: UsageAgent) -> Retained<NSColor> {
    match a {
        UsageAgent::ClaudeCode => rgba(0.941, 0.580, 0.435, 1.0),
        UsageAgent::Codex => rgba(0.557, 0.773, 1.0, 1.0),
        UsageAgent::OpenCode => rgba(0.639, 0.898, 0.632, 1.0),
        UsageAgent::Gemini => rgba(0.557, 0.639, 0.980, 1.0),
        UsageAgent::Qoder => rgba(0.980, 0.780, 0.478, 1.0),
        UsageAgent::Antigravity => rgba(0.760, 0.639, 0.980, 1.0),
    }
}

/// `95` → `"1m 35s"`, `4000` → `"1h 6m"`.
pub fn span_label(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
    }
}

/// What's left of a turn, in words: `"≈ 2 min left"`, `"under a minute"`.
pub fn eta_label(eta_s: Option<u32>, working: bool) -> String {
    match (eta_s, working) {
        (_, false) => "Waiting for you".into(),
        (None, true) => "Working".into(),
        (Some(0), true) => "Taking longer than usual".into(),
        (Some(s), true) if s < 60 => "Under a minute left".into(),
        (Some(s), true) => format!("≈ {} min left", s.div_ceil(60)),
    }
}

/// Whether a notification is this agent asking for something.
fn asks_for(n: &Notification, agent: UsageAgent) -> bool {
    if !matches!(n.kind, NotificationKind::Authorization | NotificationKind::NeedsInput) {
        return false;
    }
    let known = match agent {
        UsageAgent::ClaudeCode => Some(KnownApp::Claude),
        UsageAgent::Codex => Some(KnownApp::ChatGpt),
        UsageAgent::OpenCode => Some(KnownApp::OpenCode),
        UsageAgent::Qoder => Some(KnownApp::Qoder),
        UsageAgent::Gemini | UsageAgent::Antigravity => None,
    };
    let first_word = agent.display_name().split_whitespace().next().unwrap_or("");
    (known.is_some() && n.app.known == known) || n.app.display_name.starts_with(first_word)
}

/// The sessions to show, most in need of attention first, each with the
/// question it's waiting on.
fn ordered(snap: &Snapshot) -> Vec<(&RunningSession, Option<&Notification>)> {
    let mut asked: Vec<u64> = Vec::new();
    let mut out: Vec<(&RunningSession, Option<&Notification>)> = snap
        .usage
        .running
        .iter()
        .map(|s| {
            let q = snap.notifications.iter().find(|n| asks_for(n, s.agent) && !asked.contains(&n.id.0));
            if let Some(q) = q {
                asked.push(q.id.0);
            }
            (s, q)
        })
        .collect();
    out.sort_by_key(|(s, q)| (q.is_none(), !s.working, std::cmp::Reverse(s.last_ms)));
    out.truncate(CARDS);
    out
}

/// Where each card goes for this many sessions: one fills the page, then
/// halves, a big card and two small, and quarters.
fn card_frames(n: usize, x: f64, y: f64, w: f64, h: f64) -> Vec<NSRect> {
    let half_w = (w - GAP) / 2.0;
    let half_h = (h - GAP) / 2.0;
    match n {
        0 => vec![],
        1 => vec![rect(x, y, w, h)],
        2 => vec![rect(x, y, half_w, h), rect(x + half_w + GAP, y, half_w, h)],
        3 => vec![
            rect(x, y, half_w, h),
            rect(x + half_w + GAP, y, half_w, half_h),
            rect(x + half_w + GAP, y + half_h + GAP, half_w, half_h),
        ],
        _ => (0..4)
            .map(|i| rect(x + (i % 2) as f64 * (half_w + GAP), y + (i / 2) as f64 * (half_h + GAP), half_w, half_h))
            .collect(),
    }
}

pub struct LivePage {
    pub root: Retained<FlippedView>,
    mtm: MainThreadMarker,
    p: Palette,
    body: NSRect,
    targets: Targets,
    /// What was drawn last, so an unchanged second redraws nothing.
    key: std::cell::RefCell<String>,
}

impl LivePage {
    pub fn new(mtm: MainThreadMarker, w: f64, h: f64, nh: f64, p: &Palette) -> Self {
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        let top = nh + 6.0;
        Self {
            root,
            mtm,
            p: p.clone(),
            body: rect(PAD, top, w - 2.0 * PAD, h - top - PAD),
            targets: Targets::default(),
            key: Default::default(),
        }
    }

    pub fn update(&self, snap: &Snapshot) {
        let now = snap.now_ms;
        let sessions = ordered(snap);
        // Redraw when anything shown changes, the clock included (to the second).
        let key =
            format!("{}|{:?}", now / 1000, sessions.iter().map(|(s, q)| (s, q.map(|n| n.id.0))).collect::<Vec<_>>());
        if *self.key.borrow() == key {
            return;
        }
        self.key.replace(key);
        remove_all_subviews(&self.root);
        self.targets.clear();
        let b = self.body;
        if sessions.is_empty() {
            self.empty(b);
            return;
        }
        let frames = card_frames(sessions.len(), b.origin.x, b.origin.y, b.size.width, b.size.height);
        for ((s, q), frame) in sessions.into_iter().zip(frames) {
            let tile = glass_tile(self.mtm, frame);
            if frame.size.height > 200.0 {
                self.big_card(&tile, s, q, now);
            } else {
                self.small_card(&tile, s, q, now);
            }
            self.root.addSubview(&tile);
        }
    }

    fn empty(&self, b: NSRect) {
        let mtm = self.mtm;
        let tile = glass_tile(mtm, b);
        let (w, h) = (b.size.width, b.size.height);
        tile.addSubview(&symbol_view(
            mtm,
            "sparkles",
            26.0,
            &white(0.5),
            rect((w - 32.0) / 2.0, h / 2.0 - 44.0, 32.0, 32.0),
            "",
        ));
        for (text, y, f, c) in [
            ("No agent is working", h / 2.0 - 4.0, font(15.0, Weight::Semibold), self.p.primary.clone()),
            (
                "Sessions from Claude Code, Codex and OpenCode show here while they run.",
                h / 2.0 + 18.0,
                font(12.0, Weight::Regular),
                self.p.secondary.clone(),
            ),
        ] {
            let l = label(mtm, text, &f, &c, rect(20.0, y, w - 40.0, 18.0));
            l.setAlignment(NSTextAlignment::Center);
            tile.addSubview(&l);
        }
        self.root.addSubview(&tile);
    }

    /// Agent, project and a state chip along the top of a card.
    fn head(&self, tile: &FlippedView, s: &RunningSession, asking: bool) {
        let mtm = self.mtm;
        let w = tile.frame().size.width;
        let (state, color) = if asking {
            ("Needs you", amber())
        } else if s.working {
            ("Working", green())
        } else {
            ("Idle", white(0.6))
        };
        let chip_f = font(11.5, Weight::Semibold);
        let chip_w = crate::ui::text_width(&crate::ui::attributed(state, &chip_f, &color)) + 26.0;
        tile.addSubview(&shape_view(mtm, rect(INSET, 17.0, 9.0, 9.0), &agent_color(s.agent), 4.5));
        let (name_f, project_f) = (font(14.0, Weight::Semibold), font(12.0, Weight::Regular));
        let dim = white(0.72);
        let project = format!("  ·  {}", s.project);
        let line =
            attributed_parts(&[(s.agent.display_name(), &name_f, &self.p.primary), (&project, &project_f, &dim)]);
        tile.addSubview(&rich_label(
            mtm,
            &line,
            rect(INSET + 15.0, 12.0, w - INSET - 15.0 - chip_w - INSET - 8.0, 19.0),
        ));
        let chip = shape_view(
            mtm,
            rect(w - INSET - chip_w, 10.0, chip_w, 22.0),
            &NSColor::colorWithSRGBRed_green_blue_alpha(
                color.redComponent(),
                color.greenComponent(),
                color.blueComponent(),
                0.16,
            ),
            11.0,
        );
        chip.addSubview(&shape_view(mtm, rect(9.0, 8.0, 6.0, 6.0), &color, 3.0));
        chip.addSubview(&label(mtm, state, &chip_f, &color, rect(19.0, 3.0, chip_w - 22.0, 16.0)));
        tile.addSubview(&chip);
    }

    /// The question an agent is waiting on, with its answers.
    fn question(&self, tile: &FlippedView, q: &Notification, y: f64, h: f64, now: i64) {
        let mtm = self.mtm;
        let w = tile.frame().size.width;
        let boxv = shape_view(mtm, rect(INSET, y, w - 2.0 * INSET, h), &rgba(1.0, 0.624, 0.039, 0.12), 12.0);
        if let Some(l) = boxv.layer() {
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&rgba(1.0, 0.624, 0.039, 0.35).CGColor()));
        }
        let bw = w - 2.0 * INSET;
        let target = q.auth_target();
        let buttons: Vec<(&str, bool)> =
            if target.is_some() { vec![("Allow", true), ("Deny", false)] } else { vec![("Open", true)] };
        let mut x = bw - 10.0;
        for (title, primary) in buttons {
            let tw = if title == "Allow" { 60.0 } else { 56.0 };
            x -= tw;
            let t = match (&target, title) {
                (Some(t), _) => {
                    let t: AuthTarget = t.clone();
                    self.targets.add(mtm, move |_| {
                        with_controller(|c| c.answer_auth(&t, primary));
                    })
                }
                (None, _) => {
                    let (url, bundle) = (q.url.clone(), q.app.bundle_id.clone());
                    self.targets.add(mtm, move |_| {
                        let opened = url.as_deref().is_some_and(crate::sharing::open_url);
                        if !opened {
                            if let Some(b) = bundle.as_deref() {
                                crate::sharing::open_app(b);
                            }
                        }
                    })
                }
            };
            let bg = if primary { rgba(0.039, 0.518, 1.0, 1.0) } else { white(0.16) };
            boxv.addSubview(&pill_button(
                mtm,
                title,
                None,
                &font(12.5, Weight::Semibold),
                &NSColor::whiteColor(),
                &bg,
                13.0,
                &t,
                rect(x, (h - 26.0) / 2.0, tw, 26.0),
            ));
            x -= 8.0;
        }
        let waited = ((now - q.timestamp_ms).max(0)) / 1000;
        let ask = if q.title.trim().is_empty() { q.body.clone() } else { q.title.clone() };
        let text_w = x - 12.0;
        boxv.addSubview(&label(
            mtm,
            &format!("Waiting {}", span_label(waited)),
            &font(11.0, Weight::Semibold),
            &rgba(1.0, 0.70, 0.25, 1.0),
            rect(12.0, h / 2.0 - 17.0, text_w, 14.0),
        ));
        boxv.addSubview(&label(
            mtm,
            &ask,
            &font(13.5, Weight::Semibold),
            &self.p.primary,
            rect(12.0, h / 2.0 - 2.0, text_w, 18.0),
        ));
        set_accessibility_label(&boxv, &format!("{} asks: {ask}", q.app.display_name));
        tile.addSubview(&boxv);
    }

    /// A bar of how much of a typical turn has run.
    fn turn_bar(&self, tile: &FlippedView, s: &RunningSession, y: f64, now: i64, color: &NSColor) {
        let w = tile.frame().size.width - 2.0 * INSET;
        let track = shape_view(self.mtm, rect(INSET, y, w, 6.0), &white(0.14), 3.0);
        let elapsed = s.turn_started_ms.map_or(0, |t| (now - t).max(0) / 1000) as f64;
        let share = match s.typical_turn_s() {
            Some(t) if t > 0 => (elapsed / t as f64).min(1.0),
            _ => 0.0,
        };
        if share > 0.0 && s.working {
            track.addSubview(&shape_view(self.mtm, rect(0.0, 0.0, (w * share).max(6.0), 6.0), color, 3.0));
        }
        tile.addSubview(&track);
    }

    fn big_card(&self, tile: &FlippedView, s: &RunningSession, q: Option<&Notification>, now: i64) {
        let mtm = self.mtm;
        let size = tile.frame().size;
        let (w, h) = (size.width, size.height);
        let inner = w - 2.0 * INSET;
        self.head(tile, s, q.is_some());
        let mut y = 44.0;
        match q {
            Some(q) => {
                self.question(tile, q, y, 52.0, now);
                y += 52.0 + 12.0;
            }
            None => {
                let activity = s.activity.clone().unwrap_or_else(|| eta_label(None, s.working));
                tile.addSubview(&label(
                    mtm,
                    &activity,
                    &font(13.0, Weight::Medium),
                    &white(0.9),
                    rect(INSET, y, inner, 18.0),
                ));
                y += 28.0;
            }
        }
        // This turn against a typical one.
        let elapsed = s.turn_started_ms.map_or(0, |t| now - t) / 1000;
        let f = font(12.0, Weight::Regular);
        let strong = font(12.0, Weight::Semibold);
        let (lead, figure) = if s.working {
            ("This turn · ", span_label(elapsed))
        } else {
            ("Idle for ", span_label((now - s.last_ms) / 1000))
        };
        let line = attributed_parts(&[(lead, &f, &self.p.primary), (&figure, &strong, &self.p.primary)]);
        tile.addSubview(&rich_label(mtm, &line, rect(INSET, y, inner / 2.0, 16.0)));
        let r = label(
            mtm,
            &eta_label(s.eta_s(now), s.working),
            &strong,
            &self.p.primary,
            rect(INSET + inner / 2.0, y, inner / 2.0, 16.0),
        );
        r.setAlignment(NSTextAlignment::Right);
        tile.addSubview(&r);
        y += 22.0;
        let bar_color = if q.is_some() { amber() } else { agent_color(s.agent) };
        self.turn_bar(tile, s, y, now, &bar_color);
        y += 18.0;

        // The recent turns, as bars, in the space that's left above the footer.
        let footer_h = 56.0;
        let chart_bottom = h - INSET - footer_h - 10.0;
        if !s.recent_turns.is_empty() && chart_bottom - y > 34.0 {
            let typical =
                s.typical_turn_s().map_or(String::new(), |t| format!(" · typically {}", span_label(t as i64)));
            tile.addSubview(&label(
                mtm,
                &format!("Last {} turns{typical}", s.recent_turns.len()),
                &font(11.0, Weight::Regular),
                &white(0.72),
                rect(INSET, y, inner, 14.0),
            ));
            let top = y + 18.0;
            let ch = (chart_bottom - top).min(60.0);
            let n = s.recent_turns.len() as f64;
            let bw = (inner - 4.0 * (n - 1.0)) / n;
            let max = *s.recent_turns.iter().max().unwrap_or(&1) as f64;
            for (i, t) in s.recent_turns.iter().enumerate() {
                let bh = (*t as f64 / max.max(1.0) * ch).max(3.0);
                let bar = shape_view(
                    mtm,
                    rect(INSET + i as f64 * (bw + 4.0), top + ch - bh, bw, bh),
                    &NSColor::colorWithSRGBRed_green_blue_alpha(
                        bar_color.redComponent(),
                        bar_color.greenComponent(),
                        bar_color.blueComponent(),
                        0.7,
                    ),
                    3.0,
                );
                tile.addSubview(&bar);
            }
        }

        // Footer: how full the context is, and the session's figures.
        let fy = h - INSET - footer_h;
        let side = 52.0;
        let pct = s.context_percent.unwrap_or(0);
        let ring = ring_view(mtm, rect(INSET, fy, side, side), pct as f64 / 100.0, &self.p.primary);
        let l = label(
            mtm,
            &format!("{pct}%"),
            &mono_digits(13.0, Weight::Semibold),
            &self.p.primary,
            rect(0.0, 17.0, side, 18.0),
        );
        l.setAlignment(NSTextAlignment::Center);
        ring.addSubview(&l);
        set_accessibility_label(&ring, &format!("Context {pct}% full"));
        if s.context_percent.is_some() {
            tile.addSubview(&ring);
        }
        let sx = INSET + side + 16.0;
        let cell = (w - INSET - sx) / 3.0;
        let model = if s.model.is_empty() { "—".to_owned() } else { s.model.clone() };
        let stats = [
            ("Session", span_label(((now - s.started_ms).max(0)) / 1000)),
            ("Tokens", compact_tokens(s.tokens)),
            ("Model", model),
        ];
        for (i, (k, v)) in stats.iter().enumerate() {
            let x = sx + i as f64 * cell;
            tile.addSubview(&label(
                mtm,
                k,
                &font(10.5, Weight::Regular),
                &white(0.72),
                rect(x, fy + 9.0, cell - 6.0, 14.0),
            ));
            tile.addSubview(&label(
                mtm,
                v,
                &font(13.0, Weight::Semibold),
                &self.p.primary,
                rect(x, fy + 24.0, cell - 6.0, 18.0),
            ));
        }
        set_accessibility_label(
            tile,
            &format!("{} on {}: {}", s.agent.display_name(), s.project, eta_label(s.eta_s(now), s.working)),
        );
    }

    fn small_card(&self, tile: &FlippedView, s: &RunningSession, q: Option<&Notification>, now: i64) {
        let mtm = self.mtm;
        let size = tile.frame().size;
        let (w, h) = (size.width, size.height);
        let inner = w - 2.0 * INSET;
        self.head(tile, s, q.is_some());
        if let Some(q) = q {
            self.question(tile, q, 42.0, h - 42.0 - INSET, now);
            return;
        }
        let activity = s.activity.clone().unwrap_or_else(|| eta_label(None, s.working));
        tile.addSubview(&label(
            mtm,
            &activity,
            &font(12.5, Weight::Medium),
            &white(0.9),
            rect(INSET, 42.0, inner, 17.0),
        ));
        // Along the bottom: what's left, the figures, and the bar.
        let y = h - INSET - 6.0 - 8.0 - 16.0;
        tile.addSubview(&label(
            mtm,
            &eta_label(s.eta_s(now), s.working),
            &font(12.5, Weight::Semibold),
            &self.p.primary,
            rect(INSET, y, inner / 2.0, 16.0),
        ));
        let mut figures = Vec::new();
        if let (Some(t), Some(start)) = (s.typical_turn_s(), s.turn_started_ms.filter(|_| s.working)) {
            figures.push(format!("{}m of ~{}m", (now - start).max(0) / 60_000, (t as i64).div_euclid(60).max(1)));
        }
        if let Some(c) = s.context_percent {
            figures.push(format!("context {c}%"));
        }
        figures.push(compact_tokens(s.tokens));
        let r = label(
            mtm,
            &figures.join(" · "),
            &font(11.5, Weight::Regular),
            &white(0.72),
            rect(INSET + inner / 3.0, y, inner * 2.0 / 3.0, 16.0),
        );
        r.setAlignment(NSTextAlignment::Right);
        tile.addSubview(&r);
        self.turn_bar(tile, s, h - INSET - 6.0, now, &green());
        set_accessibility_label(
            tile,
            &format!("{} on {}: {}", s.agent.display_name(), s.project, eta_label(s.eta_s(now), s.working)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_left_reads_plainly() {
        assert_eq!(eta_label(Some(150), true), "≈ 3 min left");
        assert_eq!(eta_label(Some(30), true), "Under a minute left");
        assert_eq!(eta_label(Some(0), true), "Taking longer than usual");
        assert_eq!(eta_label(None, true), "Working");
        assert_eq!(eta_label(Some(90), false), "Waiting for you");
    }

    #[test]
    fn spans_read_short() {
        assert_eq!(span_label(42), "42s");
        assert_eq!(span_label(192), "3m 12s");
        assert_eq!(span_label(8040), "2h 14m");
    }

    #[test]
    fn cards_share_the_page_by_count() {
        assert_eq!(card_frames(1, 0.0, 0.0, 100.0, 50.0).len(), 1);
        let three = card_frames(3, 0.0, 0.0, 210.0, 110.0);
        assert_eq!(three[0].size.height, 110.0, "the first of three is tall");
        assert_eq!(three[1].size.height, 50.0);
        assert_eq!(card_frames(6, 0.0, 0.0, 210.0, 110.0).len(), 4);
    }
}
