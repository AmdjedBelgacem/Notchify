//! "Agents" page of the expanded panel: eight equal tiles, four across and two
//! down, so nothing is left over and every tile lines up with its neighbours.
//!
//! ```text
//! ╭ Usage ──╮ ╭ Tokens ─╮ ╭ By agent╮ ╭ By model╮
//! │ 238K ▂▅▇│ │ Input   │ │ ● ▓▓▓▒▒ │ │ ● ▓▓▓▒▒ │
//! ╰─────────╯ ╰─────────╯ ╰─────────╯ ╰─────────╯
//! ╭ Claude ─╮ ╭ Codex ──╮ ╭ Antigrav╮ ╭ Heaviest╮
//! │ ◯ ◯ ◯   │ │ ◯ ◯ ◯   │ │ ◯ ◯ ◯   │ │ ● Notch │
//! ╰─────────╯ ╰─────────╯ ╰─────────╯ ╰─────────╯
//! ```
//!
//! The big figures count input and output tokens; cache has its own rows,
//! because cache reads would otherwise swamp everything else.

use super::expanded::glass_tile;
use crate::app::with_controller;
use crate::ui::{
    FlippedView, Palette, Targets, Weight, font, label, mono_digits, pill_button, rect, remove_all_subviews, rgba,
    set_accessibility_label, set_text, shape_view, style_pill, symbol_view, wrapping_label,
};
use notchforge_core::model::{AgentLimits, AgentUsage, UsageAgent, UsageRange, compact_tokens};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSColor, NSTextAlignment, NSTextField};
use std::cell::{Cell, RefCell};

const PAD: f64 = 14.0;
const GAP: f64 = 10.0;
const COLUMNS: f64 = 4.0;
const ROWS: f64 = 2.0;
/// Inside a tile: its padding, and where the text under its heading starts.
const INSET: f64 = 12.0;
const BELOW_HEADING: f64 = 28.0;
/// Up to this many sessions fit a tile.
const SESSIONS_SHOWN: usize = 3;
/// Up to this many models get a row; further ones are grouped as "Other".
const MODEL_ROWS: usize = 3;
const RANGES: [(&str, &str); 3] = [("Today", "Today"), ("7D", "Last 7 days"), ("30D", "Last 30 days")];

/// A limit's colour: amber from 80 %, red from 95 %.
fn gauge_color(label: &str, percent: u8) -> Retained<NSColor> {
    if percent >= 95 {
        rgba(1.0, 0.42, 0.37, 1.0)
    } else if percent >= 80 {
        rgba(1.0, 0.624, 0.039, 1.0)
    } else if label == "Context" {
        rgba(1.0, 1.0, 1.0, 1.0)
    } else {
        rgba(0.188, 0.82, 0.345, 1.0)
    }
}

fn model_color(i: usize) -> Retained<NSColor> {
    match i {
        0 => rgba(0.941, 0.580, 0.435, 1.0),
        1 => rgba(0.557, 0.773, 1.0, 1.0),
        2 => rgba(1.0, 0.851, 0.659, 1.0),
        _ => rgba(0.725, 0.651, 1.0, 1.0),
    }
}

/// The models to list: the largest, with the rest grouped once there are too many.
pub fn model_rows(models: &[(String, u64)]) -> Vec<(String, u64)> {
    if models.len() <= MODEL_ROWS {
        return models.to_vec();
    }
    let mut rows: Vec<(String, u64)> = models[..MODEL_ROWS - 1].to_vec();
    rows.push(("Other".to_owned(), models[MODEL_ROWS - 1..].iter().map(|(_, t)| *t).sum()));
    rows
}

/// Time left until a limit resets, when that is under a day: `"2h 10m"`, `"45m"`.
pub fn time_left(resets_at_ms: i64, now_ms: i64) -> Option<String> {
    let mins = (resets_at_ms - now_ms).max(0) / 60_000;
    match mins {
        0 => Some("now".into()),
        1..=59 => Some(format!("{mins}m")),
        60..=1439 => Some(format!("{}h {}m", mins / 60, mins % 60)),
        _ => None,
    }
}

/// When a limit resets, short enough to sit under a ring: `"45m"`, `"2h 10m"`, `"3d 4h"`.
pub fn reset_label(resets_at_ms: i64, now_ms: i64) -> String {
    time_left(resets_at_ms, now_ms).unwrap_or_else(|| {
        let hours = (resets_at_ms - now_ms) / 3_600_000;
        format!("{}d {}h", hours / 24, hours % 24)
    })
}

fn short_agent_name(a: UsageAgent) -> &'static str {
    match a {
        UsageAgent::ClaudeCode => "Claude",
        UsageAgent::Codex => "Codex",
        UsageAgent::OpenCode => "OpenCode",
        UsageAgent::Gemini => "Gemini",
        UsageAgent::Qoder => "Qoder",
        UsageAgent::Antigravity => "Antigravity",
    }
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

fn white(a: f64) -> Retained<NSColor> {
    rgba(1.0, 1.0, 1.0, a)
}

/// `95` → `"1h 35m"`, `25` → `"25m"`.
pub fn duration_label(minutes: u32) -> String {
    match minutes {
        0 => "under 1m".into(),
        1..=59 => format!("{minutes}m"),
        _ => format!("{}h {}m", minutes / 60, minutes % 60),
    }
}

/// The heights of a stacked bar's segments (one per agent, [`UsageAgent::ALL`]
/// order), scaled so the tallest bar fills `h`. Anything above zero stays visible.
pub fn bar_heights(segments: &[u64], max: u64, h: f64) -> Vec<f64> {
    let scale = |v: u64| if v == 0 || max == 0 { 0.0 } else { (v as f64 / max as f64 * h).max(1.5) };
    segments.iter().map(|v| scale(*v)).collect()
}

pub struct UsagePage {
    pub root: Retained<FlippedView>,
    mtm: MainThreadMarker,
    p: Palette,
    _targets: Targets,
    usage: RefCell<AgentUsage>,
    range: Cell<usize>,
    /// Token tracking is on in Settings.
    enabled: Cell<bool>,

    tiles: Retained<FlippedView>,
    message: Retained<FlippedView>,
    message_text: Retained<NSTextField>,
    range_buttons: Vec<Retained<NSButton>>,
    hero: Retained<FlippedView>,
    kinds: Retained<FlippedView>,
    /// One per agent; `connect` shows on Claude Code's until it reports.
    limit_bodies: Vec<Retained<FlippedView>>,
    connect: Retained<NSButton>,
    by_agent: Retained<FlippedView>,
    by_model: Retained<FlippedView>,
    sessions: Retained<FlippedView>,
}

impl UsagePage {
    pub fn new(mtm: MainThreadMarker, w: f64, h: f64, nh: f64, p: &Palette) -> Self {
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        let targets = Targets::default();
        let body_y = nh + 6.0;
        let body_h = h - body_y - PAD;
        let tile_w = (w - 2.0 * PAD - (COLUMNS - 1.0) * GAP) / COLUMNS;
        let tile_h = (body_h - (ROWS - 1.0) * GAP) / ROWS;
        let cell = |col: f64, row: f64| rect(PAD + col * (tile_w + GAP), body_y + row * (tile_h + GAP), tile_w, tile_h);

        let tiles = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        root.addSubview(&tiles);

        // ── usage ──
        let hero_tile = glass_tile(mtm, cell(0.0, 0.0));
        let track = shape_view(mtm, rect(INSET, 10.0, tile_w - 2.0 * INSET, 24.0), &white(0.12), 12.0);
        hero_tile.addSubview(&track);
        let seg = (tile_w - 2.0 * INSET - 4.0) / RANGES.len() as f64;
        let range_buttons: Vec<Retained<NSButton>> = RANGES
            .iter()
            .enumerate()
            .map(|(i, (title, spoken))| {
                let t = targets.add(mtm, move |_| {
                    with_controller(|c| c.show_usage_range(i));
                });
                let b = pill_button(
                    mtm,
                    title,
                    None,
                    &font(11.0, Weight::Semibold),
                    &p.primary,
                    &white(0.0),
                    10.0,
                    &t,
                    rect(2.0 + i as f64 * seg, 2.0, seg, 20.0),
                );
                set_accessibility_label(&b, spoken);
                track.addSubview(&b);
                b
            })
            .collect();
        let hero = FlippedView::new(mtm, rect(0.0, 36.0, tile_w, tile_h - 36.0));
        hero_tile.addSubview(&hero);
        tiles.addSubview(&hero_tile);

        // ── the same tokens, by kind and by who used them ──
        let kinds = glass_tile(mtm, cell(1.0, 0.0));
        let by_agent = glass_tile(mtm, cell(2.0, 0.0));
        let by_model = glass_tile(mtm, cell(3.0, 0.0));
        tiles.addSubview(&kinds);
        tiles.addSubview(&by_agent);
        tiles.addSubview(&by_model);

        // ── limits, one tile per agent (Claude, Codex, Antigravity) ──
        let limit_tiles: Vec<_> = (0..3)
            .map(|i: usize| {
                let t = glass_tile(mtm, cell(i as f64, 1.0));
                tiles.addSubview(&t);
                t
            })
            .collect();
        let limit_bodies: Vec<_> = limit_tiles
            .iter()
            .map(|t| {
                let size = t.frame().size;
                let b = FlippedView::new(mtm, rect(0.0, 0.0, size.width, size.height));
                t.addSubview(&b);
                b
            })
            .collect();
        let t = targets.add(mtm, |_| {
            // Deferred: connecting shows an alert explaining what changed.
            dispatch2::DispatchQueue::main().exec_async(crate::app::connect_claude_limits);
        });
        let connect = pill_button(
            mtm,
            "Connect",
            None,
            &font(12.0, Weight::Semibold),
            &p.primary,
            &rgba(0.039, 0.518, 1.0, 1.0),
            13.0,
            &t,
            rect(INSET, tile_h - INSET - 26.0, tile_w - 2.0 * INSET, 26.0),
        );
        limit_tiles[0].addSubview(&connect);

        // ── heaviest sessions ──
        let sessions = glass_tile(mtm, cell(3.0, 1.0));
        tiles.addSubview(&sessions);

        // ── nothing to show yet ──
        let message = glass_tile(mtm, rect(PAD, body_y, w - 2.0 * PAD, body_h));
        message.addSubview(&symbol_view(
            mtm,
            "chart.bar.fill",
            28.0,
            &p.secondary,
            rect((w - 2.0 * PAD - 36.0) / 2.0, body_h / 2.0 - 44.0, 36.0, 36.0),
            "Token usage",
        ));
        let message_text = wrapping_label(
            mtm,
            "",
            &font(13.0, Weight::Regular),
            &p.primary,
            rect(60.0, body_h / 2.0 - 2.0, w - 2.0 * PAD - 120.0, 40.0),
            2,
        );
        message_text.setAlignment(NSTextAlignment::Center);
        message.addSubview(&message_text);
        root.addSubview(&message);

        let this = Self {
            root,
            mtm,
            p: p.clone(),
            _targets: targets,
            usage: RefCell::new(AgentUsage::default()),
            range: Cell::new(0),
            enabled: Cell::new(true),
            tiles,
            message,
            message_text,
            range_buttons,
            hero,
            kinds,
            limit_bodies,
            connect,
            by_agent,
            by_model,
            sessions,
        };
        this.render();
        this
    }

    /// New data from the provider. Returns the running count for the top row.
    pub fn update(&self, usage: &AgentUsage, enabled: bool) -> usize {
        let changed = *self.usage.borrow() != *usage || self.enabled.get() != enabled;
        if changed {
            *self.usage.borrow_mut() = usage.clone();
            self.enabled.set(enabled);
            self.render();
        }
        usage.running.len()
    }

    pub fn set_range(&self, range: usize) {
        self.range.set(range);
        self.render();
    }

    fn render(&self) {
        let usage = self.usage.borrow();
        let found = usage.claude_found
            || usage.codex_found
            || usage.opencode_found
            || usage.gemini_found
            || usage.qoder_found
            || usage.antigravity_found;
        let notice = if !self.enabled.get() {
            Some("Token usage is off. You can turn it on in Settings › Coding Agents.")
        } else if !usage.loaded {
            Some("Reading your agents' logs…")
        } else if !found {
            Some("No coding-agent logs were found on this Mac.")
        } else {
            None
        };
        self.message.setHidden(notice.is_none());
        self.tiles.setHidden(notice.is_some());
        if let Some(text) = notice {
            set_text(&self.message_text, text);
            return;
        }
        let selected = self.range.get();
        for (i, b) in self.range_buttons.iter().enumerate() {
            let bg = white(if i == selected { 0.28 } else { 0.0 });
            style_pill(b, RANGES[i].0, None, &font(11.0, Weight::Semibold), &self.p.primary, &bg, 10.0);
        }
        let empty = UsageRange::default();
        let range = usage.ranges.get(selected).unwrap_or(&empty);
        self.render_hero(range);
        self.render_kinds(range);
        self.render_limits(&usage.limits);
        self.render_breakdown(range);
        self.render_sessions(range);
    }

    fn render_hero(&self, r: &UsageRange) {
        let mtm = self.mtm;
        let p = &self.p;
        remove_all_subviews(&self.hero);
        let size = self.hero.frame().size;
        let chart_w = size.width - 2.0 * INSET;
        let total = compact_tokens(r.counts.main());
        self.hero.addSubview(&label(
            mtm,
            &total,
            &font(30.0, Weight::Light),
            &p.primary,
            rect(INSET, 0.0, chart_w, 36.0),
        ));
        let cache = compact_tokens(r.counts.cache());
        self.hero.addSubview(&label(
            mtm,
            &format!("+ {cache} cache"),
            &font(11.0, Weight::Regular),
            &white(0.76),
            rect(INSET, 35.0, chart_w, 14.0),
        ));
        set_accessibility_label(&self.hero, &format!("{total} tokens, plus {cache} from cache"));

        // Chart, with its axis along the tile's bottom edge.
        let axis_y = size.height - 8.0 - 12.0;
        let base = axis_y - 3.0;
        let chart_h = (base - 53.0).max(12.0);
        let n = r.bars.len().max(1);
        let gap = if n <= 7 { 4.0 } else { 1.5 };
        let bw = (chart_w - gap * (n - 1) as f64) / n as f64;
        let max = r.bars.iter().map(|bar| bar.iter().sum::<u64>()).max().unwrap_or(0);
        for (i, bar) in r.bars.iter().enumerate() {
            let hs = bar_heights(bar, max, chart_h);
            let x = INSET + i as f64 * (bw + gap);
            let mut y = base;
            for (slot, h) in hs.iter().enumerate() {
                if *h > 0.0 {
                    let v = shape_view(mtm, rect(x, y - h, bw, *h), &agent_color(UsageAgent::ALL[slot]), 1.0);
                    self.hero.addSubview(&v);
                    y -= h;
                }
            }
        }
        let f = font(10.0, Weight::Regular);
        let m = r.ticks.len();
        for (i, t) in r.ticks.iter().enumerate() {
            let tick_w = 26.0;
            let x = if m > 1 { INSET + i as f64 * (chart_w - tick_w) / (m - 1) as f64 } else { INSET };
            let l = label(mtm, t, &f, &white(0.74), rect(x, axis_y, tick_w, 12.0));
            l.setAlignment(if i == 0 {
                NSTextAlignment::Left
            } else if i + 1 == m {
                NSTextAlignment::Right
            } else {
                NSTextAlignment::Center
            });
            self.hero.addSubview(&l);
        }
    }

    fn render_kinds(&self, r: &UsageRange) {
        let c = &r.counts;
        let rows =
            [("Input", c.input), ("Output", c.output), ("Cache read", c.cache_read), ("Cache write", c.cache_write)];
        let tile = &self.kinds;
        remove_all_subviews(tile);
        self.heading(tile, "Tokens");
        let size = tile.frame().size;
        let w = size.width - 2.0 * INSET;
        let step = (size.height - BELOW_HEADING - 6.0) / rows.len() as f64;
        for (i, (name, value)) in rows.into_iter().enumerate() {
            let y = BELOW_HEADING + i as f64 * step;
            if i > 0 {
                tile.addSubview(&shape_view(self.mtm, rect(INSET, y, w, 0.5), &white(0.12), 0.0));
            }
            let cy = y + (step - 16.0) / 2.0;
            tile.addSubview(&label(
                self.mtm,
                name,
                &font(12.0, Weight::Regular),
                &white(0.76),
                rect(INSET, cy, w - 50.0, 16.0),
            ));
            let v = label(
                self.mtm,
                &compact_tokens(value),
                &mono_digits(13.0, Weight::Semibold),
                &self.p.primary,
                rect(size.width - INSET - 60.0, cy, 60.0, 16.0),
            );
            v.setAlignment(NSTextAlignment::Right);
            tile.addSubview(&v);
        }
    }

    fn heading(&self, host: &FlippedView, text: &str) {
        let w = host.frame().size.width;
        host.addSubview(&label(
            self.mtm,
            text,
            &font(12.0, Weight::Semibold),
            &self.p.primary,
            rect(INSET, 10.0, w - 2.0 * INSET, 15.0),
        ));
    }

    /// A short message in the middle of a tile, with its symbol above it.
    fn quiet(&self, host: &FlippedView, symbol: &str, text: &str) {
        let size = host.frame().size;
        let top = (size.height - 24.0 - 6.0 - 32.0) / 2.0;
        host.addSubview(&symbol_view(
            self.mtm,
            symbol,
            20.0,
            &white(0.5),
            rect((size.width - 24.0) / 2.0, top, 24.0, 24.0),
            "",
        ));
        let l = wrapping_label(
            self.mtm,
            text,
            &font(12.0, Weight::Regular),
            &self.p.secondary,
            rect(INSET, top + 30.0, size.width - 2.0 * INSET, 32.0),
            2,
        );
        l.setAlignment(NSTextAlignment::Center);
        host.addSubview(&l);
    }

    fn render_limits(&self, limits: &[AgentLimits]) {
        let mtm = self.mtm;
        let p = &self.p;
        let now = notchforge_core::time::now_ms();
        self.connect.setHidden(true);
        for (i, body) in self.limit_bodies.iter().enumerate() {
            remove_all_subviews(body);
            let size = body.frame().size;
            let inner = size.width - 2.0 * INSET;
            let Some(a) = limits.get(i) else {
                let text = if i == 2 { "Antigravity isn't running" } else { "No other agent found" };
                self.quiet(body, "moon.zzz", text);
                continue;
            };
            body.addSubview(&shape_view(mtm, rect(INSET, 14.5, 8.0, 8.0), &agent_color(a.agent), 4.0));
            body.addSubview(&label(
                mtm,
                a.agent.display_name(),
                &font(13.0, Weight::Semibold),
                &p.primary,
                rect(INSET + 13.0, 10.0, inner - 13.0, 17.0),
            ));
            // Whether it's running, then what it's working on or its plan.
            let f = font(11.0, Weight::Regular);
            let status = if a.running { "Running" } else { "Idle" };
            let status_color = if a.running { rgba(0.435, 0.89, 0.549, 1.0) } else { white(0.76) };
            let detail = a.project.clone().or_else(|| a.plan.clone()).map_or(String::new(), |d| format!(" · {d}"));
            let dim = white(0.76);
            let line = crate::ui::attributed_parts(&[(status, &f, &status_color), (&detail, &f, &dim)]);
            body.addSubview(&crate::ui::rich_label(mtm, &line, rect(INSET, 27.0, inner, 14.0)));

            if a.gauges.is_empty() && !a.connected {
                body.addSubview(&wrapping_label(
                    mtm,
                    "Connect to see session and weekly limits.",
                    &font(12.0, Weight::Regular),
                    &white(0.84),
                    rect(INSET, 46.0, inner, 48.0),
                    3,
                ));
                self.connect.setHidden(false);
                continue;
            }
            // Rings only for what is known: the limits, then context while running.
            let mut rings: Vec<(String, String, u8, String)> = a
                .gauges
                .iter()
                .map(|g| {
                    let short = g.label.split_whitespace().next().unwrap_or(&g.label).to_owned();
                    let under = g.resets_at_ms.map_or(String::new(), |at| reset_label(at, now));
                    (short, g.label.clone(), g.percent, under)
                })
                .collect();
            if let Some(pct) = a.context_percent {
                rings.push(("Context".into(), "Context".into(), pct, "Now".into()));
            }
            rings.truncate(3);
            // Without limits, say why under whatever ring there is.
            let why = a.gauges.is_empty().then_some(match a.agent {
                UsageAgent::ClaudeCode => "Limits need claude in a terminal.",
                UsageAgent::Codex => "No limits logged yet.",
                // Only Claude Code and Codex CLI report limits at all.
                UsageAgent::OpenCode | UsageAgent::Gemini | UsageAgent::Qoder | UsageAgent::Antigravity => {
                    "This agent doesn't report limits."
                }
            });
            self.draw_rings(body, &rings, why);
        }
    }

    /// The rings of an agent's tile, sized to fill what its header leaves. A
    /// single ring has its name beside it, with any note below.
    fn draw_rings(&self, body: &FlippedView, rings: &[(String, String, u8, String)], why: Option<&str>) {
        let mtm = self.mtm;
        let p = &self.p;
        let size = body.frame().size;
        let inner = size.width - 2.0 * INSET;
        let (top, bottom) = (44.0, size.height - 8.0);
        let room = bottom - top;
        let ring = |frame: objc2_foundation::NSRect, pct: u8, short: &str, full: &str, under: &str| {
            let side = frame.size.width;
            let view = super::expanded::ring_view(mtm, frame, pct as f64 / 100.0, &gauge_color(short, pct));
            let digits = (side * 0.27).clamp(11.0, 15.0);
            let l = label(
                mtm,
                &format!("{pct}%"),
                &mono_digits(digits, Weight::Semibold),
                &p.primary,
                rect(0.0, (side - digits - 4.0) / 2.0, side, digits + 4.0),
            );
            l.setAlignment(NSTextAlignment::Center);
            view.addSubview(&l);
            set_accessibility_label(&view, &format!("{full} {pct}% used, {under}"));
            body.addSubview(&view);
        };
        let note = |y: f64, h: f64, lines: isize| {
            if let Some(text) = why {
                let l = wrapping_label(
                    mtm,
                    text,
                    &font(11.0, Weight::Regular),
                    &white(0.76),
                    rect(INSET, y, inner, h),
                    lines,
                );
                body.addSubview(&l);
            }
        };
        match rings {
            [] => note(top, room, 3),
            [(short, full, pct, under)] => {
                let side = 54.0_f64.min(room - if why.is_some() { 34.0 } else { 0.0 });
                let ring_top = if why.is_some() { top } else { top + (room - side) / 2.0 };
                ring(rect(INSET, ring_top, side, side), *pct, short, full, under);
                let x = INSET + side + 10.0;
                let w = size.width - INSET - x;
                let mid = ring_top + side / 2.0;
                body.addSubview(&label(
                    mtm,
                    short,
                    &font(12.0, Weight::Semibold),
                    &p.primary,
                    rect(x, mid - 16.0, w, 16.0),
                ));
                body.addSubview(&label(mtm, under, &font(11.0, Weight::Regular), &white(0.76), rect(x, mid, w, 14.0)));
                note(ring_top + side + 4.0, bottom - ring_top - side - 4.0, 2);
            }
            _ => {
                let cell = (size.width - 2.0 * (INSET - 4.0)) / rings.len() as f64;
                let side = (cell - 6.0).min(room - 30.0).min(60.0);
                let block = side + 4.0 + 13.0 + 13.0;
                let ring_top = top + (room - block) / 2.0;
                for (j, (short, full, pct, under)) in rings.iter().enumerate() {
                    let x = INSET - 4.0 + j as f64 * cell;
                    ring(rect(x + (cell - side) / 2.0, ring_top, side, side), *pct, short, full, under);
                    let nl = label(
                        mtm,
                        short,
                        &font(10.5, Weight::Semibold),
                        &p.primary,
                        rect(x, ring_top + side + 4.0, cell, 13.0),
                    );
                    nl.setAlignment(NSTextAlignment::Center);
                    body.addSubview(&nl);
                    let ul = label(
                        mtm,
                        under,
                        &font(10.5, Weight::Regular),
                        &white(0.76),
                        rect(x, ring_top + side + 17.0, cell, 13.0),
                    );
                    ul.setAlignment(NSTextAlignment::Center);
                    body.addSubview(&ul);
                }
            }
        }
    }

    fn render_breakdown(&self, r: &UsageRange) {
        let agents: Vec<(String, u64)> = r.agents.iter().map(|(a, t)| (short_agent_name(*a).to_owned(), *t)).collect();
        let color_of = |name: &str| {
            r.agents.iter().find(|(a, _)| short_agent_name(*a) == name).map_or(white(0.5), |(a, _)| agent_color(*a))
        };
        let rows: Vec<(String, u64, Retained<NSColor>)> =
            model_rows(&agents).into_iter().map(|(n, t)| (n.clone(), t, color_of(&n))).collect();
        self.share_tile(&self.by_agent, "By agent", &rows);
        let rows: Vec<(String, u64, Retained<NSColor>)> =
            model_rows(&r.models).into_iter().enumerate().map(|(i, (n, t))| (n, t, model_color(i))).collect();
        self.share_tile(&self.by_model, "By model", &rows);
    }

    /// Who used the tokens: rows with a bar each, sharing the tile's height. A
    /// single row has the whole tile, as one big figure.
    fn share_tile(&self, host: &FlippedView, title: &str, rows: &[(String, u64, Retained<NSColor>)]) {
        let mtm = self.mtm;
        remove_all_subviews(host);
        self.heading(host, title);
        let size = host.frame().size;
        let w = size.width - 2.0 * INSET;
        let room = size.height - BELOW_HEADING - 8.0;
        match rows {
            [] => {
                let l = wrapping_label(
                    mtm,
                    "None in this range",
                    &font(12.0, Weight::Regular),
                    &self.p.secondary,
                    rect(INSET, BELOW_HEADING + room / 2.0 - 16.0, w, 32.0),
                    2,
                );
                l.setAlignment(NSTextAlignment::Center);
                host.addSubview(&l);
            }
            [(name, t, color)] => {
                let top = BELOW_HEADING + (room - 66.0) / 2.0;
                host.addSubview(&shape_view(mtm, rect(INSET, top + 4.0, 8.0, 8.0), color, 4.0));
                host.addSubview(&label(
                    mtm,
                    name,
                    &font(12.0, Weight::Regular),
                    &self.p.primary,
                    rect(INSET + 13.0, top, w - 13.0, 16.0),
                ));
                host.addSubview(&label(
                    mtm,
                    &compact_tokens(*t),
                    &font(26.0, Weight::Light),
                    &self.p.primary,
                    rect(INSET, top + 17.0, w, 32.0),
                ));
                host.addSubview(&label(
                    mtm,
                    "All tokens",
                    &font(11.0, Weight::Regular),
                    &white(0.76),
                    rect(INSET, top + 50.0, w, 14.0),
                ));
            }
            _ => {
                let total: u64 = rows.iter().map(|(_, t, _)| *t).sum();
                let step = room / rows.len() as f64;
                for (i, (name, t, color)) in rows.iter().enumerate() {
                    let y = BELOW_HEADING + i as f64 * step + (step - 25.0) / 2.0;
                    let share = if total > 0 { *t as f64 / total as f64 } else { 0.0 };
                    host.addSubview(&label(
                        mtm,
                        name,
                        &font(12.0, Weight::Regular),
                        &self.p.primary,
                        rect(INSET, y, w - 44.0, 16.0),
                    ));
                    let v = label(
                        mtm,
                        &compact_tokens(*t),
                        &mono_digits(12.0, Weight::Semibold),
                        &self.p.primary,
                        rect(size.width - INSET - 50.0, y, 50.0, 16.0),
                    );
                    v.setAlignment(NSTextAlignment::Right);
                    host.addSubview(&v);
                    let track = shape_view(mtm, rect(INSET, y + 19.0, w, 5.0), &white(0.14), 2.5);
                    track.addSubview(&shape_view(mtm, rect(0.0, 0.0, (w * share).max(5.0), 5.0), color, 2.5));
                    set_accessibility_label(&track, &format!("{name} {:.0}%", share * 100.0));
                    host.addSubview(&track);
                }
            }
        }
    }

    fn render_sessions(&self, r: &UsageRange) {
        let mtm = self.mtm;
        let p = &self.p;
        remove_all_subviews(&self.sessions);
        self.heading(&self.sessions, "Heaviest sessions");
        let size = self.sessions.frame().size;
        if r.sessions.is_empty() {
            self.quiet(&self.sessions, "tray", "No sessions in this range");
            return;
        }
        let w = size.width;
        let shown = r.sessions.len().min(SESSIONS_SHOWN);
        let step = (size.height - BELOW_HEADING - 6.0) / shown as f64;
        for (i, s) in r.sessions.iter().take(shown).enumerate() {
            let y = BELOW_HEADING + i as f64 * step;
            let cy = y + (step - 31.0) / 2.0;
            self.sessions.addSubview(&shape_view(mtm, rect(INSET, cy + 4.5, 7.0, 7.0), &agent_color(s.agent), 3.5));
            let value = label(
                mtm,
                &compact_tokens(s.tokens),
                &mono_digits(12.0, Weight::Semibold),
                &p.primary,
                rect(w - INSET - 44.0, cy, 44.0, 16.0),
            );
            value.setAlignment(NSTextAlignment::Right);
            self.sessions.addSubview(&value);
            self.sessions.addSubview(&label(
                mtm,
                &s.project,
                &font(12.0, Weight::Medium),
                &p.primary,
                rect(INSET + 12.0, cy, w - 2.0 * INSET - 12.0 - 46.0, 16.0),
            ));
            self.sessions.addSubview(&label(
                mtm,
                &format!("{} · {}", s.model, duration_label(s.minutes)),
                &font(10.5, Weight::Regular),
                &white(0.76),
                rect(INSET + 12.0, cy + 16.0, w - 2.0 * INSET - 12.0, 14.0),
            ));
        }
    }

    /// The top row's pill, when any session is running.
    pub fn running_label(count: usize) -> Option<String> {
        match count {
            0 => None,
            1 => Some("1 session running".into()),
            n => Some(format!("{n} sessions running")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_models_are_grouped() {
        let m = |n: &str, t| (n.to_owned(), t);
        let few = vec![m("A", 3), m("B", 2), m("C", 1)];
        assert_eq!(model_rows(&few), few);
        let many = vec![m("A", 5), m("B", 4), m("C", 3), m("D", 2)];
        assert_eq!(model_rows(&many), vec![m("A", 5), m("B", 4), m("Other", 5)]);
    }

    #[test]
    fn reset_times_read_short() {
        let min = 60_000;
        assert_eq!(time_left(45 * min, 0).as_deref(), Some("45m"));
        assert_eq!(time_left(130 * min, 0).as_deref(), Some("2h 10m"));
        assert_eq!(time_left(-5, 0).as_deref(), Some("now"));
        assert_eq!(time_left(1440 * min, 0), None, "a day or more away shows days");
        assert_eq!(reset_label(45 * min, 0), "45m");
        assert_eq!(reset_label((3 * 24 + 4) * 60 * min, 0), "3d 4h");
    }

    #[test]
    fn limits_warn_as_they_fill() {
        assert_ne!(gauge_color("Week", 83), gauge_color("Week", 79));
        assert_ne!(gauge_color("Week", 96), gauge_color("Week", 83));
    }

    #[test]
    fn durations_read_short() {
        assert_eq!(duration_label(0), "under 1m");
        assert_eq!(duration_label(25), "25m");
        assert_eq!(duration_label(134), "2h 14m");
    }

    #[test]
    fn small_bars_stay_visible() {
        assert_eq!(bar_heights(&[0, 0], 0, 50.0), vec![0.0, 0.0]);
        assert_eq!(bar_heights(&[100, 0], 100, 50.0), vec![50.0, 0.0]);
        assert_eq!(bar_heights(&[1, 0], 1000, 50.0), vec![1.5, 0.0]);
        assert_eq!(bar_heights(&[10, 20, 30], 30, 60.0), vec![20.0, 40.0, 60.0]);
    }

    #[test]
    fn pill_counts_sessions() {
        assert_eq!(UsagePage::running_label(0), None);
        assert_eq!(UsagePage::running_label(1).as_deref(), Some("1 session running"));
        assert_eq!(UsagePage::running_label(2).as_deref(), Some("2 sessions running"));
    }
}
