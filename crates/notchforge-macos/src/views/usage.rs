//! "Agents" page of the expanded panel, built to the *Pinned · Agents, token
//! usage* board of the NotchForge design canvas.
//!
//! ```text
//! ╭ (Today|7 Days|30 Days) ╮ ╭ By agent          ╮ ╭ ◯ 64%  Running now ╮
//! │ 238K tokens  + 1.60M   │ │ Claude Code  162K │ │        Codex CLI    │
//! │ ▂▅▇▃▆▇▅▃▂▅▇▄▂          │ │ ▓▓▓▓▓▓▓▓▓░░░░░░░░ │ ╰─────────────────────╯
//! ╰────────────────────────╯ ╰───────────────────╯ ╭ Heaviest sessions   ╮
//! ╭ Input  142K╮╭ Output 96K╮╭ By model          ╮ │ ● Notchify     96K  │
//! ╭ Read  1.52M╮╭ Write 84K ╮│ Opus 5.5     104K │ │ ● api-server   58K  │
//! ```
//!
//! The big figures count input and output tokens; cache has its own tiles,
//! because cache reads would otherwise swamp everything else.

use super::expanded::glass_tile;
use crate::app::with_controller;
use crate::ui::{
    FlippedView, Palette, Targets, Weight, font, label, mono_digits, pill_button, rect, remove_all_subviews, rgba,
    set_accessibility_label, set_text, shape_view, style_pill, symbol_view, wrapping_label,
};
use notchforge_core::model::{AgentUsage, UsageAgent, UsageRange, compact_tokens};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSColor, NSTextAlignment, NSTextField};
use std::cell::{Cell, RefCell};

const PAD: f64 = 14.0;
const GAP: f64 = 10.0;
const CHART_H: f64 = 54.0;

fn agent_color(a: UsageAgent) -> Retained<NSColor> {
    match a {
        UsageAgent::ClaudeCode => rgba(0.941, 0.580, 0.435, 1.0),
        UsageAgent::Codex => rgba(0.557, 0.773, 1.0, 1.0),
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

/// The heights of a stacked bar, scaled so the tallest bar fills `h`. Anything
/// above zero stays visible.
pub fn bar_heights((a, b): (u64, u64), max: u64, h: f64) -> (f64, f64) {
    let scale = |v: u64| if v == 0 || max == 0 { 0.0 } else { (v as f64 / max as f64 * h).max(1.5) };
    (scale(a), scale(b))
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
    kinds: [Retained<FlippedView>; 4],
    agents: Retained<FlippedView>,
    models: Retained<FlippedView>,
    running: Retained<FlippedView>,
    sessions: Retained<FlippedView>,
}

impl UsagePage {
    pub fn new(mtm: MainThreadMarker, w: f64, h: f64, nh: f64, p: &Palette) -> Self {
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        let targets = Targets::default();
        let body_y = nh + 6.0;
        let body_h = h - body_y - PAD;
        let avail = w - 2.0 * PAD - 2.0 * GAP;
        let c1w = (avail * 268.0 / 672.0).floor();
        let c2w = (avail * 190.0 / 672.0).floor();
        let c3w = avail - c1w - c2w;
        let c2x = PAD + c1w + GAP;
        let c3x = c2x + c2w + GAP;

        let tiles = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        root.addSubview(&tiles);

        // ── usage ──
        let hero_h = (body_h * 0.57).round();
        let hero_tile = glass_tile(mtm, rect(PAD, body_y, c1w, hero_h));
        let track = shape_view(mtm, rect(12.0, 10.0, c1w - 24.0, 26.0), &white(0.12), 13.0);
        hero_tile.addSubview(&track);
        let seg = (c1w - 24.0 - 4.0) / 3.0;
        let range_buttons: Vec<Retained<NSButton>> = ["Today", "7 Days", "30 Days"]
            .iter()
            .enumerate()
            .map(|(i, title)| {
                let t = targets.add(mtm, move |_| {
                    with_controller(|c| c.show_usage_range(i));
                });
                let b = pill_button(
                    mtm,
                    title,
                    None,
                    &font(12.0, Weight::Semibold),
                    &p.primary,
                    &white(0.0),
                    11.0,
                    &t,
                    rect(2.0 + i as f64 * seg, 2.0, seg, 22.0),
                );
                track.addSubview(&b);
                b
            })
            .collect();
        let hero = FlippedView::new(mtm, rect(0.0, 36.0, c1w, hero_h - 36.0));
        hero_tile.addSubview(&hero);
        tiles.addSubview(&hero_tile);

        let kinds_y = body_y + hero_h + GAP;
        let kind_h = (body_h - hero_h - 2.0 * GAP) / 2.0;
        let kind_w = (c1w - GAP) / 2.0;
        let kinds = [0, 1, 2, 3].map(|i: usize| {
            let (col, row) = ((i % 2) as f64, (i / 2) as f64);
            let t = glass_tile(mtm, rect(PAD + col * (kind_w + GAP), kinds_y + row * (kind_h + GAP), kind_w, kind_h));
            if let Some(l) = t.layer() {
                l.setCornerRadius(16.0);
            }
            tiles.addSubview(&t);
            t
        });

        // ── by agent, by model ──
        let agents_h = ((body_h - GAP) / 2.0).round();
        let agents = glass_tile(mtm, rect(c2x, body_y, c2w, agents_h));
        let models = glass_tile(mtm, rect(c2x, body_y + agents_h + GAP, c2w, body_h - agents_h - GAP));
        tiles.addSubview(&agents);
        tiles.addSubview(&models);

        // ── running now, heaviest sessions ──
        let running_h = 92.0;
        let running = glass_tile(mtm, rect(c3x, body_y, c3w, running_h));
        let sessions = glass_tile(mtm, rect(c3x, body_y + running_h + GAP, c3w, body_h - running_h - GAP));
        tiles.addSubview(&running);
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
            agents,
            models,
            running,
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
        let found = usage.claude_found || usage.codex_found;
        let notice = if !self.enabled.get() {
            Some("Token usage is off. You can turn it on in Settings › Coding Agents.")
        } else if !usage.loaded {
            Some("Reading your agents' logs…")
        } else if !found {
            Some("No Claude Code or Codex CLI logs were found on this Mac.")
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
            let title = ["Today", "7 Days", "30 Days"][i];
            let bg = white(if i == selected { 0.28 } else { 0.0 });
            style_pill(b, title, None, &font(12.0, Weight::Semibold), &self.p.primary, &bg, 11.0);
        }
        let empty = UsageRange::default();
        let range = usage.ranges.get(selected).unwrap_or(&empty);
        self.render_hero(range);
        self.render_kinds(range);
        self.render_agents(range, usage.opencode_found);
        self.render_models(range);
        self.render_running(&usage);
        self.render_sessions(range);
    }

    fn render_hero(&self, r: &UsageRange) {
        let mtm = self.mtm;
        let p = &self.p;
        remove_all_subviews(&self.hero);
        let size = self.hero.frame().size;
        let total = compact_tokens(r.counts.main());
        let big = font(34.0, Weight::Light);
        let tw = crate::ui::text_width(&crate::ui::attributed(&total, &big, &p.primary));
        self.hero.addSubview(&label(mtm, &total, &big, &p.primary, rect(12.0, 2.0, tw + 6.0, 40.0)));
        self.hero.addSubview(&label(
            mtm,
            "tokens",
            &font(12.0, Weight::Regular),
            &white(0.8),
            rect(12.0 + tw + 6.0, 20.0, 50.0, 16.0),
        ));
        let cache = label(
            mtm,
            &format!("+ {} cache", compact_tokens(r.counts.cache())),
            &font(12.0, Weight::Regular),
            &white(0.8),
            rect(size.width - 112.0, 20.0, 100.0, 16.0),
        );
        cache.setAlignment(NSTextAlignment::Right);
        self.hero.addSubview(&cache);
        set_accessibility_label(
            &self.hero,
            &format!("{} tokens, plus {} from cache", total, compact_tokens(r.counts.cache())),
        );

        // Chart, with its axis along the tile's bottom edge.
        let axis_y = size.height - 8.0 - 13.0;
        let base = axis_y - 3.0;
        let chart_h = CHART_H.min(base - 44.0).max(16.0);
        let chart_w = size.width - 24.0;
        let n = r.bars.len().max(1);
        let gap = if n <= 7 { 6.0 } else { 2.0 };
        let bw = (chart_w - gap * (n - 1) as f64) / n as f64;
        let max = r.bars.iter().map(|(a, b)| a + b).max().unwrap_or(0);
        for (i, bar) in r.bars.iter().enumerate() {
            let (ha, hb) = bar_heights(*bar, max, chart_h);
            let x = 12.0 + i as f64 * (bw + gap);
            if ha > 0.0 {
                let v = shape_view(mtm, rect(x, base - ha, bw, ha), &agent_color(UsageAgent::ClaudeCode), 1.5);
                self.hero.addSubview(&v);
            }
            if hb > 0.0 {
                let v = shape_view(mtm, rect(x, base - ha - hb, bw, hb), &agent_color(UsageAgent::Codex), 1.5);
                self.hero.addSubview(&v);
            }
        }
        let f = font(11.0, Weight::Regular);
        let m = r.ticks.len();
        for (i, t) in r.ticks.iter().enumerate() {
            let tick_w = 28.0;
            let x = if m > 1 { 12.0 + i as f64 * (chart_w - tick_w) / (m - 1) as f64 } else { 12.0 };
            let l = label(mtm, t, &f, &white(0.74), rect(x, axis_y, tick_w, 13.0));
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
        for (tile, (name, value)) in self.kinds.iter().zip(rows) {
            remove_all_subviews(tile);
            let size = tile.frame().size;
            let top = ((size.height - 39.0) / 2.0).max(4.0);
            tile.addSubview(&label(
                self.mtm,
                name,
                &font(12.0, Weight::Regular),
                &white(0.8),
                rect(12.0, top, size.width - 24.0, 15.0),
            ));
            tile.addSubview(&label(
                self.mtm,
                &compact_tokens(value),
                &font(19.0, Weight::Semibold),
                &self.p.primary,
                rect(12.0, top + 15.0, size.width - 24.0, 24.0),
            ));
        }
    }

    /// A name, a figure and a bar, as used for agents and models.
    fn meter(&self, host: &FlippedView, y: f64, name: &str, (tokens, share): (u64, f64), color: &NSColor, thick: f64) {
        let w = host.frame().size.width;
        let value = compact_tokens(tokens);
        let f = font(13.0, Weight::Medium);
        host.addSubview(&label(self.mtm, name, &f, &self.p.primary, rect(12.0, y, w - 72.0, 17.0)));
        let v = label(
            self.mtm,
            &value,
            &mono_digits(13.0, Weight::Semibold),
            &self.p.primary,
            rect(w - 72.0, y, 60.0, 17.0),
        );
        v.setAlignment(NSTextAlignment::Right);
        host.addSubview(&v);
        let track = shape_view(self.mtm, rect(12.0, y + 20.0, w - 24.0, thick), &white(0.18), thick / 2.0);
        if let Some(l) = track.layer() {
            l.setMasksToBounds(true);
        }
        track.addSubview(&shape_view(self.mtm, rect(0.0, 0.0, (w - 24.0) * share, thick), color, 0.0));
        host.addSubview(&track);
    }

    fn heading(&self, host: &FlippedView, text: &str) {
        let w = host.frame().size.width;
        host.addSubview(&label(
            self.mtm,
            text,
            &font(12.0, Weight::Semibold),
            &self.p.primary,
            rect(12.0, 10.0, w - 24.0, 15.0),
        ));
    }

    fn quiet(&self, host: &FlippedView, text: &str) {
        let size = host.frame().size;
        let l = wrapping_label(
            self.mtm,
            text,
            &font(12.0, Weight::Regular),
            &self.p.secondary,
            rect(12.0, (size.height + 12.0) / 2.0 - 16.0, size.width - 24.0, 32.0),
            2,
        );
        l.setAlignment(NSTextAlignment::Center);
        host.addSubview(&l);
    }

    fn render_agents(&self, r: &UsageRange, opencode: bool) {
        remove_all_subviews(&self.agents);
        self.heading(&self.agents, "By agent");
        let size = self.agents.frame().size;
        let total: u64 = r.agents.iter().map(|(_, t)| *t).sum();
        if r.agents.is_empty() {
            self.quiet(&self.agents, "No agent used tokens in this range");
        }
        let mut y = 34.0;
        for (agent, tokens) in &r.agents {
            let share = if total > 0 { *tokens as f64 / total as f64 } else { 0.0 };
            self.meter(&self.agents, y, agent.display_name(), (*tokens, share), &agent_color(*agent), 7.0);
            y += 37.0;
        }
        if opencode {
            self.agents.addSubview(&label(
                self.mtm,
                "OpenCode not tracked",
                &font(12.0, Weight::Regular),
                &white(0.76),
                rect(12.0, size.height - 10.0 - 15.0, size.width - 24.0, 15.0),
            ));
        }
    }

    fn render_models(&self, r: &UsageRange) {
        remove_all_subviews(&self.models);
        self.heading(&self.models, "By model");
        let size = self.models.frame().size;
        let total: u64 = r.models.iter().map(|(_, t)| *t).sum();
        if r.models.is_empty() {
            self.quiet(&self.models, "No model used in this range");
            return;
        }
        let shown = r.models.iter().take(3);
        let n = r.models.len().min(3) as f64;
        // Spread the rows over the tile rather than leaving a gap below them.
        let step = ((size.height - 30.0 - 8.0) / n).min(44.0);
        for (i, (name, tokens)) in shown.enumerate() {
            let share = if total > 0 { *tokens as f64 / total as f64 } else { 0.0 };
            self.meter(&self.models, 30.0 + i as f64 * step, name, (*tokens, share), &white(0.96), 4.0);
        }
    }

    fn render_running(&self, usage: &AgentUsage) {
        let mtm = self.mtm;
        let p = &self.p;
        remove_all_subviews(&self.running);
        let size = self.running.frame().size;
        let Some(s) = usage.running.first() else {
            self.running.addSubview(&symbol_view(
                mtm,
                "moon.zzz",
                22.0,
                &p.secondary,
                rect(14.0, (size.height - 30.0) / 2.0, 34.0, 30.0),
                "",
            ));
            self.running.addSubview(&label(
                mtm,
                "No session running",
                &font(13.0, Weight::Medium),
                &p.primary,
                rect(56.0, (size.height - 17.0) / 2.0, size.width - 66.0, 17.0),
            ));
            return;
        };
        let side = 56.0;
        let y0 = (size.height - side) / 2.0;
        let color = agent_color(s.agent);
        let tx = match s.context_percent {
            Some(pct) => {
                let ring = super::expanded::ring_view(mtm, rect(12.0, y0, side, side), pct as f64 / 100.0, &color);
                let l = label(
                    mtm,
                    &format!("{pct}%"),
                    &mono_digits(14.0, Weight::Semibold),
                    &p.primary,
                    rect(0.0, 18.0, side, 20.0),
                );
                l.setAlignment(NSTextAlignment::Center);
                ring.addSubview(&l);
                set_accessibility_label(&ring, &format!("Context {pct}% full"));
                self.running.addSubview(&ring);
                12.0 + side + 12.0
            }
            None => {
                // Without a known window size, the figure is this session's tokens.
                let l = label(
                    mtm,
                    &compact_tokens(s.tokens),
                    &font(24.0, Weight::Light),
                    &p.primary,
                    rect(12.0, (size.height - 30.0) / 2.0, 72.0, 30.0),
                );
                self.running.addSubview(&l);
                88.0
            }
        };
        let tw = size.width - tx - 8.0;
        let top = (size.height - 49.0) / 2.0;
        self.running.addSubview(&label(
            mtm,
            "Running now",
            &font(12.0, Weight::Semibold),
            &rgba(0.435, 0.89, 0.549, 1.0),
            rect(tx, top, tw, 15.0),
        ));
        self.running.addSubview(&label(
            mtm,
            s.agent.display_name(),
            &font(14.0, Weight::Semibold),
            &p.primary,
            rect(tx, top + 15.0, tw, 19.0),
        ));
        let detail = if s.context_percent.is_some() { format!("Context · {}", s.project) } else { s.project.clone() };
        self.running.addSubview(&label(
            mtm,
            &detail,
            &font(12.0, Weight::Regular),
            &white(0.8),
            rect(tx, top + 34.0, tw, 15.0),
        ));
    }

    fn render_sessions(&self, r: &UsageRange) {
        let mtm = self.mtm;
        let p = &self.p;
        remove_all_subviews(&self.sessions);
        self.heading(&self.sessions, "Heaviest sessions");
        let size = self.sessions.frame().size;
        if r.sessions.is_empty() {
            self.quiet(&self.sessions, "No sessions in this range");
            return;
        }
        let w = size.width;
        let top = 28.0;
        let step = ((size.height - top - 6.0) / 4.0).max(32.0);
        for (i, s) in r.sessions.iter().enumerate() {
            let y = top + i as f64 * step;
            let cy = y + (step - 32.0) / 2.0;
            self.sessions.addSubview(&shape_view(mtm, rect(12.0, cy + 12.0, 8.0, 8.0), &agent_color(s.agent), 4.0));
            let value = label(
                mtm,
                &compact_tokens(s.tokens),
                &mono_digits(13.0, Weight::Semibold),
                &p.primary,
                rect(w - 62.0, cy, 50.0, 17.0),
            );
            value.setAlignment(NSTextAlignment::Right);
            self.sessions.addSubview(&value);
            self.sessions.addSubview(&label(
                mtm,
                &s.project,
                &font(13.0, Weight::Medium),
                &p.primary,
                rect(28.0, cy, w - 28.0 - 66.0, 17.0),
            ));
            self.sessions.addSubview(&label(
                mtm,
                &format!("{} · {}", s.model, duration_label(s.minutes)),
                &font(12.0, Weight::Regular),
                &white(0.76),
                rect(28.0, cy + 17.0, w - 28.0 - 10.0, 15.0),
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
    fn durations_read_short() {
        assert_eq!(duration_label(0), "under 1m");
        assert_eq!(duration_label(25), "25m");
        assert_eq!(duration_label(134), "2h 14m");
    }

    #[test]
    fn small_bars_stay_visible() {
        assert_eq!(bar_heights((0, 0), 0, 50.0), (0.0, 0.0));
        assert_eq!(bar_heights((100, 0), 100, 50.0), (50.0, 0.0));
        assert_eq!(bar_heights((1, 0), 1000, 50.0), (1.5, 0.0));
    }

    #[test]
    fn pill_counts_sessions() {
        assert_eq!(UsagePage::running_label(0), None);
        assert_eq!(UsagePage::running_label(1).as_deref(), Some("1 session running"));
        assert_eq!(UsagePage::running_label(2).as_deref(), Some("2 sessions running"));
    }
}
