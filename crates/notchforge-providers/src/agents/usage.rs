//! Token usage of coding agents, read from the session logs they already keep:
//! Claude Code in `~/.claude/projects`, Codex CLI in `~/.codex/sessions`.
//!
//! Only token counts, model names, times and the project folder's name are
//! kept. The logs also hold whole conversations, which are never stored or
//! sent anywhere. Files are read once and then only from where the last read
//! stopped, because the logs can run to gigabytes.

use notchforge_core::model::{
    AgentLimits, AgentUsage, LimitGauge, ProviderStatus, RunningSession, TokenCounts, UsageAgent, UsageRange,
    UsageSession, limit_label,
};
use notchforge_core::provider::{
    BoxFuture, Provider, ProviderCommand, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub struct AgentUsageProvider;

const POLL: Duration = Duration::from_secs(30);
const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;
/// The longest range shown, plus a day so a range never starts part-read.
const KEEP_DAYS: i64 = 31;
/// A session idle longer than this drops off the Live page.
const LIVE_MS: i64 = 30 * 60_000;
/// Turns kept per session to judge how long the next one takes.
const TURNS_KEPT: usize = 8;
/// A session that logged tokens this recently counts as running.
const RUNNING_MS: i64 = 180_000;
const TOP_SESSIONS: usize = 3;
/// Claude Code's status data says which session it came from for this long.
const STATUS_FRESH_MS: i64 = 600_000;

/// Parse an RFC 3339 UTC time like `2026-11-01T18:03:22.326Z` to Unix ms.
pub fn parse_time(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    // Howard Hinnant's days-from-civil.
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let millis = match b.get(19) {
        Some(b'.') => s.get(20..23).and_then(|m| m.parse::<i64>().ok()).unwrap_or(0),
        _ => 0,
    };
    Some(days * DAY_MS + (h * 3600 + mi * 60 + sec) * 1000 + millis)
}

/// `claude-opus-5-5-20260101` → `Opus 5.5`, `gpt-5-codex` → `GPT-5 Codex`.
pub fn model_label(id: &str) -> String {
    let parts: Vec<&str> = id.split('-').filter(|p| !(p.len() == 8 && p.chars().all(|c| c.is_ascii_digit()))).collect();
    let title = |p: &str| {
        let mut c = p.chars();
        c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect::<String>())
    };
    match parts.as_slice() {
        ["claude", family, version @ ..] if !version.is_empty() => format!("{} {}", title(family), version.join(".")),
        ["gpt", version, rest @ ..] => {
            let words: Vec<String> = rest.iter().map(|p| title(p)).collect();
            format!("GPT-{version} {}", words.join(" ")).trim().to_owned()
        }
        _ => id.to_owned(),
    }
}

/// Claude Code's logs don't say how large a session's context window is. Opus
/// runs with the 1M window, and anything past 200K must have it too; otherwise
/// assume the default. Its status line reports the exact figure when connected.
fn claude_window(model: &str, used: u64) -> u64 {
    if model.contains("opus") || used > 200_000 { 1_000_000 } else { 200_000 }
}

/// Time spent working: gaps longer than this between messages are idle.
const IDLE_GAP_MS: i64 = 15 * 60_000;

/// Minutes of activity across message times, which must be sorted.
pub fn active_minutes(times: &[i64]) -> u32 {
    let ms: i64 = times.windows(2).map(|w| w[1] - w[0]).filter(|gap| *gap <= IDLE_GAP_MS).sum();
    (ms / 60_000) as u32
}

fn project_name(cwd: &str) -> String {
    Path::new(cwd).file_name().and_then(|n| n.to_str()).unwrap_or(cwd).to_owned()
}

#[derive(Deserialize)]
struct ClaudeLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Lines the CLI adds itself, not something the person typed.
    #[serde(rename = "isMeta", default)]
    is_meta: bool,
    timestamp: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    /// A sub-agent's message: it has its own context, not the session's.
    #[serde(rename = "isSidechain", default)]
    is_sidechain: bool,
    message: Option<ClaudeMessage>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ClaudeUsage>,
    content: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

#[derive(Deserialize)]
struct CodexLine {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: Option<serde_json::Value>,
}

struct Record {
    ts_ms: i64,
    session: Arc<str>,
    model: Arc<str>,
    agent: UsageAgent,
    counts: TokenCounts,
}

/// A session's tokens in a range, its agent, and its tokens per model.
type SessionTally<'a> = (u64, UsageAgent, HashMap<&'a str, u64>);

#[derive(Default)]
struct SessionInfo {
    project: String,
    last_ms: i64,
    /// (tokens in use, window) as of `context_ms`.
    context: Option<(u64, u64)>,
    context_ms: i64,
    first_ms: i64,
    model: String,
    /// The current turn: when it began, and its latest line.
    turn_start_ms: Option<i64>,
    turn_last_ms: i64,
    /// Seconds each recent turn took, oldest first.
    turns: Vec<u32>,
    activity: Option<String>,
}

impl SessionInfo {
    /// The person sent a message: the turn before it is over.
    fn begin_turn(&mut self, ts: i64) {
        if let Some(start) = self.turn_start_ms {
            if ts <= start {
                return;
            }
            if self.turn_last_ms > start {
                self.turns.push(((self.turn_last_ms - start) / 1000) as u32);
                if self.turns.len() > TURNS_KEPT {
                    self.turns.remove(0);
                }
            }
        }
        self.turn_start_ms = Some(ts);
        self.turn_last_ms = ts;
        self.activity = None;
    }

    fn note_work(&mut self, ts: i64, activity: Option<String>) {
        if self.turn_start_ms.is_some_and(|s| ts >= s) {
            self.turn_last_ms = self.turn_last_ms.max(ts);
        }
        if activity.is_some() && ts >= self.turn_last_ms {
            self.activity = activity;
        }
    }
}

/// The file a tool call works on, by its name only.
fn file_name(input: &serde_json::Value) -> Option<String> {
    let path = input.get("file_path").or(input.get("path")).or(input.get("notebook_path"))?.as_str()?;
    Some(path.rsplit('/').next().unwrap_or(path).to_owned())
}

/// What an assistant message does, in a few words, from its last tool call.
pub fn claude_activity(content: &serde_json::Value) -> Option<String> {
    let parts = content.as_array()?;
    let tool = parts.iter().rev().find(|p| p.get("type").and_then(|t| t.as_str()) == Some("tool_use"));
    let Some(tool) = tool else {
        return parts
            .iter()
            .any(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
            .then(|| "Writing a reply".to_owned());
    };
    let name = tool.get("name").and_then(|n| n.as_str()).unwrap_or("a tool");
    let input = tool.get("input").cloned().unwrap_or_default();
    let short = |s: &str| -> String {
        let s = s.lines().next().unwrap_or("").trim();
        if s.chars().count() > 48 { format!("{}…", s.chars().take(47).collect::<String>()) } else { s.to_owned() }
    };
    Some(match name {
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
            format!("Editing {}", file_name(&input).unwrap_or_else(|| "a file".into()))
        }
        "Read" => format!("Reading {}", file_name(&input).unwrap_or_else(|| "a file".into())),
        "Bash" => match input.get("description").and_then(|d| d.as_str()) {
            Some(d) if !d.is_empty() => short(d),
            _ => format!("Running {}", short(input.get("command").and_then(|c| c.as_str()).unwrap_or("a command"))),
        },
        "Grep" | "Glob" => "Searching the code".into(),
        "WebFetch" | "WebSearch" => "Searching the web".into(),
        "Task" | "Agent" => "Working with a sub-agent".into(),
        "TodoWrite" => "Planning the next steps".into(),
        other => format!("Using {other}"),
    })
}

/// Is this user line something the person typed, rather than a tool's result?
fn is_prompt(content: &serde_json::Value) -> bool {
    match content {
        serde_json::Value::String(s) => !s.trim().is_empty(),
        serde_json::Value::Array(parts) => {
            let kind = |p: &serde_json::Value| p.get("type").and_then(|t| t.as_str()).map(str::to_owned);
            parts.iter().all(|p| kind(p).as_deref() != Some("tool_result"))
                && parts.iter().any(|p| kind(p).as_deref() == Some("text"))
        }
        _ => false,
    }
}

/// A Codex rate limit entry, or `None` where it has no such window.
fn codex_gauge(v: &serde_json::Value) -> Option<LimitGauge> {
    let percent = v.get("used_percent")?.as_f64()?;
    Some(LimitGauge {
        label: limit_label(v.get("window_minutes")?.as_u64()?),
        percent: percent.round().clamp(0.0, 100.0) as u8,
        resets_at_ms: v.get("resets_at").and_then(|r| r.as_i64()).map(|s| s * 1000),
    })
}

/// The latest limits Codex logged.
#[derive(Default, Clone)]
struct CodexLimits {
    ts_ms: i64,
    plan: Option<String>,
    gauges: Vec<LimitGauge>,
}

/// What Claude Code's status line last reported, read from its saved JSON.
#[derive(Debug, Default, PartialEq)]
pub struct ClaudeStatus {
    pub gauges: Vec<LimitGauge>,
    pub context_percent: Option<u8>,
    pub project: Option<String>,
}

/// Parse the status line's JSON. A window whose reset time has passed is gone.
pub fn parse_claude_status(text: &str, now_ms: i64) -> Option<ClaudeStatus> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let window = |key: &str, label: &str| -> Option<LimitGauge> {
        let w = v.get("rate_limits")?.get(key)?;
        let resets_at_ms = w.get("resets_at")?.as_i64()? * 1000;
        if resets_at_ms <= now_ms {
            return None;
        }
        let percent = w.get("used_percentage")?.as_f64()?.round().clamp(0.0, 100.0) as u8;
        Some(LimitGauge { label: label.to_owned(), percent, resets_at_ms: Some(resets_at_ms) })
    };
    Some(ClaudeStatus {
        gauges: [window("five_hour", "Session"), window("seven_day", "Week")].into_iter().flatten().collect(),
        context_percent: v
            .get("context_window")
            .and_then(|c| c.get("used_percentage"))
            .and_then(|p| p.as_f64())
            .map(|p| p.round().clamp(0.0, 100.0) as u8),
        project: v
            .get("workspace")
            .and_then(|w| w.get("project_dir").or(w.get("current_dir")))
            .or(v.get("cwd"))
            .and_then(|d| d.as_str())
            .map(project_name),
    })
}

#[derive(Default)]
struct FileState {
    offset: u64,
    /// Codex states these once, ahead of the token counts that use them.
    model: Option<Arc<str>>,
    session: Option<Arc<str>>,
}

/// What a single OpenCode session's cumulative totals looked like when last
/// read: each poll counts only the growth.
#[derive(Default, Clone)]
struct OpenCodeSeen {
    updated: i64,
    totals: TokenCounts,
}

/// Everything read from the logs so far.
#[derive(Default)]
pub struct Scanner {
    files: HashMap<PathBuf, FileState>,
    records: HashMap<String, Record>,
    sessions: HashMap<Arc<str>, SessionInfo>,
    strings: HashMap<String, Arc<str>>,
    codex_limits: Option<CodexLimits>,
    /// Claude Code logs a limit when it is hit: (window, resets at).
    claude_hit: Option<(String, i64)>,
    /// OpenCode's per-session cumulative totals, from its SQLite database.
    opencode: HashMap<String, OpenCodeSeen>,
}

/// The short key an agent is named by in session and record keys.
fn agent_key(agent: UsageAgent) -> &'static str {
    match agent {
        UsageAgent::ClaudeCode => "claude",
        UsageAgent::Codex => "codex",
        UsageAgent::OpenCode => "opencode",
        UsageAgent::Gemini => "gemini",
        UsageAgent::Qoder => "qoder",
        UsageAgent::Antigravity => "antigravity",
    }
}

impl Scanner {
    fn intern(&mut self, s: &str) -> Arc<str> {
        if let Some(a) = self.strings.get(s) {
            return a.clone();
        }
        let a: Arc<str> = Arc::from(s);
        self.strings.insert(s.to_owned(), a.clone());
        a
    }

    fn touch_session(&mut self, key: &Arc<str>, ts_ms: i64, project: Option<&str>) {
        let info = self
            .sessions
            .entry(key.clone())
            .or_insert_with(|| SessionInfo { last_ms: ts_ms, first_ms: ts_ms, ..Default::default() });
        if ts_ms > 0 && (info.first_ms == 0 || ts_ms < info.first_ms) {
            info.first_ms = ts_ms;
        }
        info.last_ms = info.last_ms.max(ts_ms);
        if let Some(p) = project.filter(|p| !p.is_empty()) {
            if info.project.is_empty() {
                info.project = project_name(p);
            }
        }
    }

    /// One Claude Code log line. A message is logged again as it streams, so
    /// it is counted once, with its largest output. Resumed and sub-agent logs
    /// repeat message ids too, so this holds across files.
    pub fn add_claude_line(&mut self, line: &str) {
        if line.contains("\"quotaLimits\"") {
            self.note_claude_limit(line);
        }
        // Something the person typed starts a turn; a tool's result is part of one.
        if line.contains("\"type\":\"user\"") && !line.contains("\"tool_result\"") {
            self.note_claude_prompt(line);
            return;
        }
        if !line.contains("\"usage\"") {
            return;
        }
        let Ok(l) = serde_json::from_str::<ClaudeLine>(line) else { return };
        let Some(msg) = l.message else { return };
        let activity = msg.content.as_ref().and_then(claude_activity);
        let (Some(id), Some(usage), Some(ts)) = (msg.id, msg.usage, l.timestamp.as_deref().and_then(parse_time)) else {
            return;
        };
        let model = msg.model.unwrap_or_default();
        // Placeholder entries the CLI writes itself cost nothing.
        if model.is_empty() || model.starts_with('<') {
            return;
        }
        let counts = TokenCounts {
            input: usage.input_tokens,
            output: usage.output_tokens,
            cache_read: usage.cache_read_input_tokens,
            cache_write: usage.cache_creation_input_tokens,
        };
        let session_key = format!("claude:{}", l.session_id.unwrap_or_default());
        let session = self.intern(&session_key);
        let model = self.intern(&model);
        self.touch_session(&session, ts, l.cwd.as_deref());
        if let Some(info) = self.sessions.get_mut(&session) {
            info.note_work(ts, if l.is_sidechain { None } else { activity });
            if !l.is_sidechain && !model.is_empty() {
                info.model = model.to_string();
            }
        }
        // What the session's context holds is everything sent with its latest message.
        if !l.is_sidechain {
            if let Some(info) = self.sessions.get_mut(&session).filter(|i| ts >= i.context_ms) {
                let used = counts.input + counts.cache_read + counts.cache_write;
                info.context = Some((used, claude_window(&model, used)));
                info.context_ms = ts;
            }
        }
        let key = format!("claude:{id}");
        match self.records.get_mut(&key) {
            Some(r) => {
                if counts.output >= r.counts.output {
                    r.counts = counts;
                }
            }
            None => {
                self.records.insert(key, Record { ts_ms: ts, session, model, agent: UsageAgent::ClaudeCode, counts });
            }
        }
    }

    /// A message the person typed into Claude Code: a new turn begins.
    fn note_claude_prompt(&mut self, line: &str) {
        let Ok(l) = serde_json::from_str::<ClaudeLine>(line) else { return };
        if l.kind.as_deref() != Some("user") || l.is_meta || l.is_sidechain {
            return;
        }
        let Some(ts) = l.timestamp.as_deref().and_then(parse_time) else { return };
        if !l.message.as_ref().and_then(|m| m.content.as_ref()).is_some_and(is_prompt) {
            return;
        }
        let session = self.intern(&format!("claude:{}", l.session_id.unwrap_or_default()));
        self.touch_session(&session, ts, l.cwd.as_deref());
        if let Some(info) = self.sessions.get_mut(&session) {
            info.begin_turn(ts);
        }
    }

    /// A refused request says which limit was reached and when it resets.
    fn note_claude_limit(&mut self, line: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return };
        let Some(q) = v.get("quotaLimits") else { return };
        if q.get("status").and_then(|s| s.as_str()) != Some("rejected") {
            return;
        }
        let (Some(kind), Some(resets)) =
            (q.get("rateLimitType").and_then(|t| t.as_str()), q.get("resetsAt").and_then(|r| r.as_i64()))
        else {
            return;
        };
        let label = if kind == "seven_day" { "Week" } else { "Session" };
        if self.claude_hit.as_ref().is_none_or(|(_, at)| resets * 1000 >= *at) {
            self.claude_hit = Some((label.to_owned(), resets * 1000));
        }
    }

    /// One Codex CLI log line. Its `input_tokens` already include cached ones.
    fn add_codex_line(&mut self, line: &str, state: &mut FileState, fallback: &str) {
        if !(line.contains("token_count")
            || line.contains("session_meta")
            || line.contains("turn_context")
            || line.contains("user_message")
            || line.contains("function_call"))
        {
            return;
        }
        let Ok(l) = serde_json::from_str::<CodexLine>(line) else { return };
        let Some(payload) = l.payload else { return };
        let ts = l.timestamp.as_deref().and_then(parse_time);
        match l.kind.as_deref() {
            Some("session_meta") => {
                let id = payload.get("id").and_then(|v| v.as_str()).unwrap_or(fallback);
                let key = self.intern(&format!("codex:{id}"));
                let cwd = payload.get("cwd").and_then(|v| v.as_str());
                self.touch_session(&key, ts.unwrap_or(0), cwd);
                state.session = Some(key);
            }
            Some("turn_context") => {
                if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
                    state.model = Some(self.intern(m));
                }
            }
            // The person's message starts a turn; a command the agent runs is its activity.
            Some("event_msg") if payload.get("type").and_then(|v| v.as_str()) == Some("user_message") => {
                let (Some(ts), Some(session)) = (ts, state.session.clone()) else { return };
                if let Some(info) = self.sessions.get_mut(&session) {
                    info.begin_turn(ts);
                }
            }
            Some("response_item") if payload.get("type").and_then(|v| v.as_str()) == Some("function_call") => {
                let (Some(ts), Some(session)) = (ts, state.session.clone()) else { return };
                let args: serde_json::Value = payload
                    .get("arguments")
                    .and_then(|a| a.as_str())
                    .and_then(|a| serde_json::from_str(a).ok())
                    .unwrap_or_default();
                let command = args.get("command").map(|c| match c {
                    serde_json::Value::Array(parts) => {
                        parts.iter().filter_map(|p| p.as_str()).collect::<Vec<_>>().join(" ")
                    }
                    other => other.as_str().unwrap_or("").to_owned(),
                });
                let activity = match command.filter(|c| !c.is_empty()) {
                    Some(c) => {
                        let c = c.trim_start_matches("bash -lc ").trim_matches(['\'', '"']);
                        let c: String = c.chars().take(48).collect();
                        format!("Running {c}")
                    }
                    None => "Working".to_owned(),
                };
                if let Some(info) = self.sessions.get_mut(&session) {
                    info.note_work(ts, Some(activity));
                }
            }
            Some("event_msg") if payload.get("type").and_then(|v| v.as_str()) == Some("token_count") => {
                let Some(ts) = ts else { return };
                if let Some(r) = payload.get("rate_limits").filter(|r| !r.is_null()) {
                    if self.codex_limits.as_ref().is_none_or(|l| ts >= l.ts_ms) {
                        let gauges =
                            [r.get("primary"), r.get("secondary")].into_iter().flatten().filter_map(codex_gauge);
                        self.codex_limits = Some(CodexLimits {
                            ts_ms: ts,
                            plan: r.get("plan_type").and_then(|p| p.as_str()).map(str::to_owned),
                            gauges: gauges.collect(),
                        });
                    }
                }
                let Some(info) = payload.get("info").filter(|i| !i.is_null()) else { return };
                let num = |o: &serde_json::Value, k: &str| o.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                let Some(last) = info.get("last_token_usage") else { return };
                let Some(total) = info.get("total_token_usage").map(|t| num(t, "total_tokens")) else { return };
                let session = match &state.session {
                    Some(s) => s.clone(),
                    None => {
                        let key = self.intern(&format!("codex:{fallback}"));
                        state.session = Some(key.clone());
                        key
                    }
                };
                let model = state.model.clone().unwrap_or_else(|| self.intern("codex"));
                self.touch_session(&session, ts, None);
                if let Some(info) = self.sessions.get_mut(&session) {
                    info.note_work(ts, None);
                    info.model = model.to_string();
                }
                let input = num(last, "input_tokens");
                let cached = num(last, "cached_input_tokens");
                if let (Some(window), Some(info)) =
                    (info.get("model_context_window").and_then(|v| v.as_u64()), self.sessions.get_mut(&session))
                {
                    info.context = Some((input, window));
                }
                // The same running total is logged again when nothing was used.
                let key = format!("{session}:{total}");
                self.records.entry(key).or_insert(Record {
                    ts_ms: ts,
                    session,
                    model,
                    agent: UsageAgent::Codex,
                    counts: TokenCounts {
                        input: input.saturating_sub(cached),
                        output: num(last, "output_tokens"),
                        cache_read: cached,
                        cache_write: num(last, "cache_write_input_tokens"),
                    },
                });
            }
            _ => {}
        }
    }

    /// Read whatever a log gained since the last look.
    fn read_file(&mut self, path: &Path, agent: UsageAgent) {
        let Ok(meta) = std::fs::metadata(path) else { return };
        let mut state = self.files.remove(path).unwrap_or_default();
        if meta.len() < state.offset {
            state = FileState::default();
        }
        if meta.len() > state.offset {
            let fallback = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_owned();
            if let Err(e) = self.read_lines(path, agent, &mut state, &fallback) {
                tracing::debug!(path = %path.display(), error = %e, "could not read an agent log");
            }
        }
        self.files.insert(path.to_owned(), state);
    }

    /// Usage an agent reported through the transports instead of writing a log
    /// (Gemini, Qoder, Antigravity — anything that can run a command).
    pub fn note_usage(
        &mut self,
        agent: UsageAgent,
        model: Option<&str>,
        session: Option<&str>,
        counts: TokenCounts,
        at_ms: i64,
        project: Option<&str>,
    ) {
        if counts == TokenCounts::default() {
            return;
        }
        let session_key =
            self.intern(&format!("{}:{}", agent_key(agent), session.filter(|s| !s.is_empty()).unwrap_or("session")));
        self.touch_session(&session_key, at_ms, project);
        let model = self.intern(model.filter(|m| !m.is_empty()).unwrap_or("unknown"));
        // Same millisecond and size means the same report was retried.
        let key = format!("{}:{at_ms}:{}", agent_key(agent), counts.main());
        self.records.insert(key, Record { ts_ms: at_ms, session: session_key, model, agent, counts });
    }

    /// OpenCode keeps *cumulative* token totals per session in its SQLite
    /// database (`session_v2`), refreshed as the session runs. Each session is
    /// re-read whenever it changes and only the growth is counted, so a
    /// session can be resumed days later and still land in the right range.
    fn read_opencode(&mut self, db: &Path, cutoff: i64) {
        use rusqlite::OpenFlags;
        if !db.exists() {
            return;
        }
        let Ok(conn) = rusqlite::Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            return;
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT s.id, s.directory, s.model, s.tokens_input, s.tokens_output, s.tokens_reasoning, \
             s.tokens_cache_read, s.tokens_cache_write, s.time_updated \
             FROM session_v2 s WHERE s.time_updated > ?1 ORDER BY s.time_updated ASC",
        ) else {
            // A future schema change: OpenCode stays untracked rather than crashing.
            tracing::info!("OpenCode's database no longer has the expected tables; skipping it");
            return;
        };
        let rows = stmt
            .query_map([cutoff], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, u64>(3)?,
                    r.get::<_, u64>(4)?,
                    r.get::<_, u64>(5)?,
                    r.get::<_, u64>(6)?,
                    r.get::<_, u64>(7)?,
                    r.get::<_, i64>(8)?,
                ))
            })
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>());
        let Ok(rows) = rows else {
            tracing::debug!("could not read OpenCode's database");
            return;
        };
        tracing::info!(db = %db.display(), sessions = rows.len(), "read OpenCode sessions");
        for (id, directory, model_raw, input, output, reasoning, cache_read, cache_write, updated) in rows {
            // Reasoning tokens are generated output.
            let output = output + reasoning;
            let seen = self.opencode.entry(id.clone()).or_default();
            if updated <= seen.updated {
                continue;
            }
            let totals = TokenCounts { input, output, cache_read, cache_write };
            let delta = TokenCounts {
                input: totals.input.saturating_sub(seen.totals.input),
                output: totals.output.saturating_sub(seen.totals.output),
                cache_read: totals.cache_read.saturating_sub(seen.totals.cache_read),
                cache_write: totals.cache_write.saturating_sub(seen.totals.cache_write),
            };
            seen.updated = updated;
            seen.totals = totals;
            if delta == TokenCounts::default() {
                continue;
            }
            // `model` is a JSON blob like {"id":"gemini-3-pro","providerID":"…"}.
            let model = model_raw
                .as_deref()
                .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
                .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_owned))
                .or(model_raw)
                .unwrap_or_default();
            let session_key = self.intern(&format!("opencode:{id}"));
            self.touch_session(&session_key, updated, Some(&directory));
            let record = Record {
                ts_ms: updated,
                session: session_key,
                model: self.intern(&model),
                agent: UsageAgent::OpenCode,
                counts: delta,
            };
            self.records.insert(format!("opencode:{id}:{updated}"), record);
        }
    }

    fn read_lines(
        &mut self,
        path: &Path,
        agent: UsageAgent,
        state: &mut FileState,
        fallback: &str,
    ) -> std::io::Result<()> {
        let mut file = std::fs::File::open(path)?;
        file.seek(SeekFrom::Start(state.offset))?;
        let mut reader = BufReader::with_capacity(256 * 1024, file);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let n = reader.read_until(b'\n', &mut buf)?;
            // A line still being written is read again next time.
            if n == 0 || buf.last() != Some(&b'\n') {
                return Ok(());
            }
            state.offset += n as u64;
            let Ok(line) = std::str::from_utf8(&buf) else { continue };
            match agent {
                UsageAgent::ClaudeCode => self.add_claude_line(line),
                UsageAgent::Codex => self.add_codex_line(line, state, fallback),
                // The other agents are read from databases or reported via the
                // transports, not from JSONL logs.
                _ => {}
            }
        }
    }

    /// Bring everything up to date and forget what no range can show any more.
    pub fn scan(&mut self, home: &Path, now_ms: i64) {
        let cutoff = now_ms - KEEP_DAYS * DAY_MS;
        for (root, agent) in
            [(home.join(".claude/projects"), UsageAgent::ClaudeCode), (home.join(".codex/sessions"), UsageAgent::Codex)]
        {
            for path in recent_logs(&root, cutoff) {
                self.read_file(&path, agent);
            }
        }
        self.read_opencode(&home.join(".local/share/opencode/opencode.db"), cutoff);
        self.records.retain(|_, r| r.ts_ms >= cutoff);
        self.sessions.retain(|_, s| s.last_ms >= cutoff);
        self.opencode.retain(|_, s| s.updated >= cutoff);
    }

    /// The three ranges the page shows, ending today.
    pub fn summarize(&self, today_start_ms: i64) -> Vec<UsageRange> {
        let hours = self.range("Today", today_start_ms, 24, HOUR_MS);
        let week = self.range("7 Days", today_start_ms - 6 * DAY_MS, 7, DAY_MS);
        let month = self.range("30 Days", today_start_ms - 29 * DAY_MS, 30, DAY_MS);
        vec![hours, week, month]
    }

    fn range(&self, label: &str, start_ms: i64, buckets: usize, step_ms: i64) -> UsageRange {
        let mut out = UsageRange {
            label: label.to_owned(),
            bars: vec![vec![0; UsageAgent::ALL.len()]; buckets],
            ..Default::default()
        };
        let mut agents: HashMap<UsageAgent, u64> = HashMap::new();
        let mut models: HashMap<&str, u64> = HashMap::new();
        // Per session: tokens, and tokens per model to name its main one.
        let mut sessions: HashMap<&Arc<str>, SessionTally> = HashMap::new();
        let mut times: HashMap<&Arc<str>, Vec<i64>> = HashMap::new();
        for r in self.records.values() {
            if r.ts_ms < start_ms {
                continue;
            }
            let i = ((r.ts_ms - start_ms) / step_ms) as usize;
            if i >= buckets {
                continue;
            }
            let tokens = r.counts.main();
            out.counts.add(&r.counts);
            out.bars[i][r.agent.slot()] += tokens;
            *agents.entry(r.agent).or_default() += tokens;
            *models.entry(&r.model).or_default() += tokens;
            let s = sessions.entry(&r.session).or_insert((0, r.agent, HashMap::new()));
            s.0 += tokens;
            times.entry(&r.session).or_default().push(r.ts_ms);
            *s.2.entry(&r.model).or_default() += tokens;
        }
        out.agents = agents.into_iter().collect();
        out.agents.sort_by_key(|(a, t)| (std::cmp::Reverse(*t), *a));
        out.models = models.into_iter().map(|(m, t)| (model_label(m), t)).collect();
        out.models.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut list: Vec<UsageSession> = sessions
            .into_iter()
            .map(|(key, (tokens, agent, by_model))| {
                let info = self.sessions.get(key);
                let model = by_model.into_iter().max_by_key(|(_, t)| *t).map(|(m, _)| m).unwrap_or_default();
                UsageSession {
                    project: info
                        .map(|i| i.project.clone())
                        .filter(|p| !p.is_empty())
                        .unwrap_or_else(|| "Unknown".into()),
                    agent,
                    model: model_label(model),
                    minutes: times.get_mut(key).map_or(0, |t| {
                        t.sort_unstable();
                        active_minutes(t)
                    }),
                    tokens,
                }
            })
            .collect();
        list.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.project.cmp(&b.project)));
        list.truncate(TOP_SESSIONS);
        out.sessions = list;
        out
    }

    /// Each found agent's limits. `claude` is what its status line last said,
    /// and whether it is connected.
    pub fn limits(
        &self,
        now_ms: i64,
        running: &[RunningSession],
        (claude_found, codex_found): (bool, bool),
        claude: Option<ClaudeStatus>,
        claude_connected: bool,
    ) -> Vec<AgentLimits> {
        let live = |agent| running.iter().find(|r| r.agent == agent && r.working);
        let mut out = Vec::new();
        if claude_found {
            let s = live(UsageAgent::ClaudeCode);
            let status = claude.unwrap_or_default();
            let mut gauges = status.gauges;
            // Without the status line, a limit that was hit is still worth showing.
            if let Some((label, at)) = self.claude_hit.as_ref().filter(|(_, at)| *at > now_ms) {
                if !gauges.iter().any(|g| &g.label == label) {
                    gauges.push(LimitGauge { label: label.clone(), percent: 100, resets_at_ms: Some(*at) });
                }
            }
            out.push(AgentLimits {
                agent: UsageAgent::ClaudeCode,
                connected: claude_connected,
                plan: None,
                running: s.is_some(),
                project: s.map(|r| r.project.clone()).or(status.project),
                gauges,
                // The status line's exact figure, else what the logs show.
                context_percent: s.and(status.context_percent).or(s.and_then(|r| r.context_percent)),
            });
        }
        if codex_found {
            let s = live(UsageAgent::Codex);
            let limits = self.codex_limits.clone().unwrap_or_default();
            let gauges = limits.gauges.into_iter().filter(|g| g.resets_at_ms.is_none_or(|at| at > now_ms)).collect();
            out.push(AgentLimits {
                agent: UsageAgent::Codex,
                connected: true,
                plan: limits.plan.map(|p| {
                    let mut c = p.chars();
                    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect::<String>()) + " plan"
                }),
                running: s.is_some(),
                project: s.map(|r| r.project.clone()),
                gauges,
                context_percent: s.and_then(|r| r.context_percent),
            });
        }
        out
    }

    /// Sessions active in the last half hour, latest first. Those that logged
    /// something in the last few minutes are working.
    pub fn running(&self, now_ms: i64) -> Vec<RunningSession> {
        let mut tokens: HashMap<&Arc<str>, (u64, UsageAgent)> = HashMap::new();
        for r in self.records.values() {
            let e = tokens.entry(&r.session).or_insert((0, r.agent));
            e.0 += r.counts.main();
        }
        let mut live: Vec<(i64, RunningSession)> = self
            .sessions
            .iter()
            .filter(|(_, s)| now_ms - s.last_ms <= LIVE_MS)
            .filter_map(|(key, s)| {
                let (t, agent) = tokens.get(key)?;
                Some((
                    s.last_ms,
                    RunningSession {
                        agent: *agent,
                        project: if s.project.is_empty() { "Unknown".into() } else { s.project.clone() },
                        tokens: *t,
                        context_percent: s
                            .context
                            .filter(|(_, w)| *w > 0)
                            .map(|(used, w)| ((used as f64 / w as f64) * 100.0).round().min(100.0) as u8),
                        working: now_ms - s.last_ms <= RUNNING_MS,
                        model: model_label(&s.model),
                        started_ms: s.first_ms,
                        turn_started_ms: s.turn_start_ms,
                        last_ms: s.last_ms,
                        recent_turns: s.turns.clone(),
                        activity: s.activity.clone(),
                    },
                ))
            })
            .collect();
        live.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
        live.into_iter().map(|(_, r)| r).collect()
    }
}

/// `.jsonl` files under `root` changed since `cutoff`.
fn recent_logs(root: &Path, cutoff_ms: i64) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let Ok(kind) = e.file_type() else { continue };
            let path = e.path();
            if kind.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "jsonl") {
                let fresh = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .is_some_and(|d| d.as_millis() as i64 >= cutoff_ms);
                if fresh {
                    out.push(path);
                }
            }
        }
    }
    out
}

/// Start of the local day holding `now_ms`.
fn local_day_start(now_ms: i64) -> i64 {
    let secs = (now_ms / 1000) as libc::time_t;
    // SAFETY: both pointers refer to valid, initialised locals for the call.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return now_ms - now_ms.rem_euclid(DAY_MS);
        }
        tm.tm_hour = 0;
        tm.tm_min = 0;
        tm.tm_sec = 0;
        libc::mktime(&mut tm) as i64 * 1000
    }
}

/// (day of month, weekday with Sunday 0) of a local time.
fn local_day(ms: i64) -> (i32, i32) {
    let secs = (ms / 1000) as libc::time_t;
    // SAFETY: both pointers refer to valid, initialised locals for the call.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return (1, 0);
        }
        (tm.tm_mday, tm.tm_wday)
    }
}

/// Axis labels under each range's chart.
fn add_ticks(ranges: &mut [UsageRange], today_start_ms: i64) {
    const LETTERS: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];
    if let [today, week, month] = ranges {
        today.ticks = ["00", "06", "12", "18", "24"].map(str::to_owned).to_vec();
        week.ticks =
            (0..7).rev().map(|d| LETTERS[local_day(today_start_ms - d * DAY_MS).1 as usize].to_owned()).collect();
        month.ticks = [29, 22, 15, 7, 0].iter().map(|d| local_day(today_start_ms - d * DAY_MS).0.to_string()).collect();
    }
}

/// The status line's saved JSON, if Claude Code wrote it recently enough to
/// say which session is current. Its limits hold until they reset regardless.
fn read_claude_status(data_dir: &Path, now_ms: i64) -> Option<ClaudeStatus> {
    let path = data_dir.join(super::connect::CLAUDE_STATUS_FILE);
    let mut status = parse_claude_status(&std::fs::read_to_string(&path).ok()?, now_ms)?;
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .is_some_and(|d| now_ms - d.as_millis() as i64 <= STATUS_FRESH_MS);
    if !fresh {
        status.context_percent = None;
    }
    Some(status)
}

fn read_usage(scanner: &mut Scanner, home: &Path, data_dir: &Path, now_ms: i64) -> AgentUsage {
    let started = std::time::Instant::now();
    scanner.scan(home, now_ms);
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        records = scanner.records.len(),
        "agent usage read"
    );
    let today = local_day_start(now_ms);
    let mut ranges = scanner.summarize(today);
    add_ticks(&mut ranges, today);
    let running = scanner.running(now_ms);
    let claude_found = home.join(".claude/projects").is_dir();
    let codex_found = home.join(".codex/sessions").is_dir();
    let env = super::connect::Env { home: home.to_owned(), notify_bin: None, port: 0, token: None };
    let connected = super::connect::claude_limits_status(&env) == super::connect::Status::Connected;
    let limits =
        scanner.limits(now_ms, &running, (claude_found, codex_found), read_claude_status(data_dir, now_ms), connected);
    AgentUsage {
        loaded: true,
        claude_found,
        codex_found,
        opencode_found: home.join(".local/share/opencode").is_dir(),
        gemini_found: home.join(".gemini").is_dir(),
        qoder_found: home.join(".qoder").is_dir() || home.join("Library/Application Support/Qoder").is_dir(),
        antigravity_found: home.join(".gemini/antigravity").is_dir(),
        ranges,
        running,
        limits,
    }
}

impl Provider for AgentUsageProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "agent-usage", name: "Agent token usage", permissions: &[] }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
                ctx.set_status(ProviderStatus::Failing("No home folder".into())).await;
                ctx.shutdown.wait().await;
                return Ok(());
            };
            ctx.set_status(ProviderStatus::Running).await;
            let mut scanner = Scanner::default();
            let mut last: Option<AgentUsage> = None;
            loop {
                let now = notchforge_core::time::now_ms();
                let h = home.clone();
                let data = ctx.paths.data_dir.clone();
                // The first read can cover gigabytes: keep it off the runtime.
                let Ok((back, usage)) = tokio::task::spawn_blocking(move || {
                    let usage = read_usage(&mut scanner, &h, &data, now);
                    (scanner, usage)
                })
                .await
                else {
                    return Err(ProviderError::Other("reading agent logs stopped unexpectedly".into()));
                };
                scanner = back;
                if last.as_ref() != Some(&usage) {
                    if !ctx.emit(ProviderEvent::Usage(usage.clone())).await {
                        return Ok(());
                    }
                    last = Some(usage);
                }
                let mut shutdown = ctx.shutdown.clone();
                tokio::select! {
                    _ = shutdown.wait() => return Ok(()),
                    _ = tokio::time::sleep(POLL) => {}
                    cmd = ctx.next_command() => {
                        // Usage reported through the transports: count it and
                        // read everything again right away.
                        if let Some(ProviderCommand::Usage { agent, model, session, counts, at_ms }) = cmd {
                            scanner.note_usage(agent, model.as_deref(), session.as_deref(), counts, at_ms, None);
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "2026-11-01T10:30:00.500Z";

    #[test]
    fn reads_opencodes_cumulative_totals_incrementally() {
        let dir = std::env::temp_dir().join(format!("nf-opencode-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("opencode.db");
        let conn = rusqlite::Connection::open(&db).expect("fixture db");
        conn.execute_batch(
            "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, directory TEXT NOT NULL, model TEXT, \
             tokens_input INTEGER DEFAULT 0, tokens_output INTEGER DEFAULT 0, tokens_reasoning INTEGER DEFAULT 0, \
             tokens_cache_read INTEGER DEFAULT 0, tokens_cache_write INTEGER DEFAULT 0, time_updated INTEGER NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_v2 (id, directory, model, tokens_input, tokens_output, tokens_reasoning, \
             tokens_cache_read, tokens_cache_write, time_updated) \
             VALUES ('ses_1', '/work/deep-space', '{\"id\":\"nemotron-3.5\",\"providerID\":\"openrouter\"}', \
             1000, 200, 50, 3000, 0, 5000)",
            [],
        )
        .unwrap();

        let mut sc = Scanner::default();
        sc.read_opencode(&db, 0);
        assert_eq!(sc.records.len(), 1);
        let r = sc.records.values().next().unwrap();
        assert_eq!(r.agent, UsageAgent::OpenCode);
        assert_eq!(r.counts.input, 1000);
        assert_eq!(r.counts.output, 250, "reasoning folds into output");
        assert_eq!(r.counts.cache_read, 3000);
        assert_eq!(r.model.to_string(), "nemotron-3.5");
        let s = sc.sessions.values().next().unwrap();
        assert_eq!(s.project, "deep-space");

        // The same state again adds nothing.
        sc.read_opencode(&db, 0);
        assert_eq!(sc.records.len(), 1);

        // Growth counts only the growth.
        conn.execute(
            "UPDATE session_v2 SET tokens_input = 1600, tokens_output = 400, tokens_reasoning = 100, \
             time_updated = 9000 WHERE id = 'ses_1'",
            [],
        )
        .unwrap();
        sc.read_opencode(&db, 0);
        assert_eq!(sc.records.len(), 2);
        let latest = sc.records.values().max_by_key(|r| r.ts_ms).unwrap();
        assert_eq!((latest.counts.input, latest.counts.output), (600, 250));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reported_usage_counts_without_a_log() {
        let mut sc = Scanner::default();
        sc.note_usage(
            UsageAgent::Gemini,
            Some("gemini-3-flash"),
            Some("sess-9"),
            TokenCounts { input: 1200, output: 340, ..Default::default() },
            12345,
            Some("/work/apollo"),
        );
        assert_eq!(sc.records.len(), 1);
        let r = sc.records.values().next().unwrap();
        assert_eq!(r.agent, UsageAgent::Gemini);
        assert_eq!(r.model.to_string(), "gemini-3-flash");
        let s = sc.sessions.values().next().unwrap();
        assert_eq!(s.project, "apollo");

        // An identical retry (same millisecond and size) counts once.
        sc.note_usage(
            UsageAgent::Gemini,
            Some("gemini-3-flash"),
            Some("sess-9"),
            TokenCounts { input: 1200, output: 340, ..Default::default() },
            12345,
            None,
        );
        assert_eq!(sc.records.len(), 1);
    }

    fn claude(id: &str, output: u64, session: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{T}","cwd":"/Users/me/Notchify","sessionId":"{session}","message":{{"id":"{id}","model":"claude-opus-5-5","content":[{{"text":"private words"}}],"usage":{{"input_tokens":10,"output_tokens":{output},"cache_read_input_tokens":100,"cache_creation_input_tokens":5}}}}}}"#
        )
    }

    #[test]
    fn parses_log_times() {
        assert_eq!(parse_time("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_time("2026-11-01T18:03:22.326Z"), Some(1_793_556_202_326));
        assert_eq!(parse_time("2024-02-29T00:00:01Z"), Some(1_709_164_801_000));
        assert_eq!(parse_time("yesterday"), None);
    }

    #[test]
    fn names_models_for_people() {
        assert_eq!(model_label("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_label("claude-sonnet-5-5-20260101"), "Sonnet 5.5");
        assert_eq!(model_label("gpt-5-codex"), "GPT-5 Codex");
        assert_eq!(model_label("gpt-6-luna"), "GPT-6 Luna");
        assert_eq!(model_label("o4"), "o4");
    }

    #[test]
    fn counts_a_streamed_claude_message_once() {
        let mut s = Scanner::default();
        s.add_claude_line(&claude("msg_1", 2, "a"));
        s.add_claude_line(&claude("msg_1", 40, "a"));
        s.add_claude_line(&claude("msg_1", 40, "resumed-copy"));
        s.add_claude_line(&claude("msg_2", 8, "a"));
        let now = parse_time(T).unwrap();
        let day = now - now.rem_euclid(DAY_MS);
        let r = &s.summarize(day)[0];
        assert_eq!(r.counts, TokenCounts { input: 20, output: 48, cache_read: 200, cache_write: 10 });
        assert_eq!(r.bars[10], vec![68, 0, 0, 0, 0, 0], "cache stays out of the bars");
        assert_eq!(r.agents, vec![(UsageAgent::ClaudeCode, 68)]);
        assert_eq!(r.models, vec![("Opus 5.5".to_owned(), 68)]);
        assert_eq!(r.sessions[0].project, "Notchify");
        assert_eq!(r.sessions[0].tokens, 68);
    }

    #[test]
    fn times_turns_and_notes_what_a_session_is_doing() {
        let at = |secs: i64| format!("2026-11-07T10:{:02}:{:02}.000Z", secs / 60, secs % 60);
        let prompt = |secs| {
            format!(
                r#"{{"type":"user","timestamp":"{}","sessionId":"s1","cwd":"/x/Notchify","message":{{"role":"user","content":"go"}}}}"#,
                at(secs)
            )
        };
        let reply = |secs, id: &str, tool: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{}","sessionId":"s1","message":{{"id":"{id}","model":"claude-opus-5-5","content":[{tool}],"usage":{{"input_tokens":1,"output_tokens":1}}}}}}"#,
                at(secs)
            )
        };
        let edit = r#"{"type":"tool_use","name":"Edit","input":{"file_path":"/x/src/main.rs"}}"#;
        let result = r#"{"type":"user","timestamp":"2026-11-07T10:00:50.000Z","sessionId":"s1","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#;
        let mut s = Scanner::default();
        s.add_claude_line(&prompt(0));
        s.add_claude_line(&reply(30, "m1", edit));
        s.add_claude_line(result);
        s.add_claude_line(&reply(90, "m2", r#"{"type":"text","text":"done"}"#));
        s.add_claude_line(&prompt(120));
        s.add_claude_line(&reply(150, "m3", edit));
        let now = parse_time(&at(160)).unwrap();
        let live = &s.running(now)[0];
        assert_eq!(live.recent_turns, vec![90], "a tool's result doesn't start a turn");
        assert_eq!(live.turn_started_ms, parse_time(&at(120)));
        assert_eq!(live.activity.as_deref(), Some("Editing main.rs"));
        assert!(live.working);
    }

    #[test]
    fn estimates_what_a_turn_has_left() {
        let r = RunningSession {
            working: true,
            turn_started_ms: Some(0),
            recent_turns: vec![100, 300, 200],
            ..Default::default()
        };
        assert_eq!(r.typical_turn_s(), Some(200));
        assert_eq!(r.eta_s(50_000), Some(150));
        assert_eq!(r.eta_s(500_000), Some(0), "taking longer than usual");
        assert_eq!(RunningSession { recent_turns: vec![100], ..r.clone() }.eta_s(0), None, "too few turns to tell");
    }

    #[test]
    fn ignores_lines_without_usage_and_placeholders() {
        let mut s = Scanner::default();
        s.add_claude_line(r#"{"type":"user","message":{"content":"the word usage here"}}"#);
        s.add_claude_line("not json with \"usage\"");
        let synthetic = claude("msg_3", 0, "a").replace("claude-opus-5-5", "<synthetic>");
        s.add_claude_line(&synthetic);
        assert!(s.records.is_empty());
    }

    #[test]
    fn splits_cached_tokens_out_of_codex_input() {
        let mut s = Scanner::default();
        let mut st = FileState::default();
        let meta =
            format!(r#"{{"timestamp":"{T}","type":"session_meta","payload":{{"id":"s1","cwd":"/work/api-server"}}}}"#);
        let turn = format!(r#"{{"timestamp":"{T}","type":"turn_context","payload":{{"model":"gpt-5-codex"}}}}"#);
        let tokens = format!(
            r#"{{"timestamp":"{T}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":1100}},"last_token_usage":{{"input_tokens":1000,"cached_input_tokens":900,"cache_write_input_tokens":0,"output_tokens":100,"total_tokens":1100}},"model_context_window":2000}}}}}}"#
        );
        for l in [&meta, &turn, &tokens, &tokens] {
            s.add_codex_line(l, &mut st, "file");
        }
        let now = parse_time(T).unwrap();
        let r = &s.summarize(now - now.rem_euclid(DAY_MS))[0];
        assert_eq!(r.counts, TokenCounts { input: 100, output: 100, cache_read: 900, cache_write: 0 });
        assert_eq!(r.models[0].0, "GPT-5 Codex");
        let live = s.running(now + 1000);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].project, "api-server");
        assert_eq!(live[0].context_percent, Some(50));
        assert!(s.running(now + RUNNING_MS + 1).iter().all(|r| !r.working), "quiet a while: no longer working");
        assert!(s.running(now + LIVE_MS + 1).is_empty(), "quiet for half an hour: gone");
    }

    #[test]
    fn reads_only_what_a_file_gained() {
        let dir = std::env::temp_dir().join(format!("nf-usage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let projects = dir.join(".claude/projects/p");
        std::fs::create_dir_all(&projects).unwrap();
        let log = projects.join("a.jsonl");
        std::fs::write(&log, format!("{}\n", claude("m1", 5, "a"))).unwrap();
        let mut s = Scanner::default();
        s.scan(&dir, parse_time(T).unwrap());
        assert_eq!(s.records.len(), 1);
        let offset = s.files[&log].offset;
        // A half-written line waits until it is complete.
        std::fs::write(&log, format!("{}\n{}", claude("m1", 5, "a"), claude("m2", 5, "a"))).unwrap();
        s.scan(&dir, parse_time(T).unwrap());
        assert_eq!(s.records.len(), 1);
        assert_eq!(s.files[&log].offset, offset);
        std::fs::write(&log, format!("{}\n{}\n", claude("m1", 5, "a"), claude("m2", 5, "a"))).unwrap();
        s.scan(&dir, parse_time(T).unwrap());
        assert_eq!(s.records.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_claude_limits_from_its_status_data() {
        let json = r#"{"workspace":{"project_dir":"/work/Notchify"},
            "context_window":{"used_percentage":18.4},
            "rate_limits":{"five_hour":{"used_percentage":41.6,"resets_at":2000},
                           "seven_day":{"used_percentage":83,"resets_at":500}}}"#;
        let s = parse_claude_status(json, 1_000_000).expect("parse");
        assert_eq!(s.project.as_deref(), Some("Notchify"));
        assert_eq!(s.context_percent, Some(18));
        assert_eq!(s.gauges.len(), 1, "a window past its reset is gone");
        assert_eq!(s.gauges[0], LimitGauge { label: "Session".into(), percent: 42, resets_at_ms: Some(2_000_000) });
        assert_eq!(parse_claude_status("{}", 0).expect("empty").gauges, vec![]);
        assert_eq!(parse_claude_status("nope", 0), None);
    }

    #[test]
    fn keeps_the_latest_codex_limits() {
        let mut s = Scanner::default();
        let mut st = FileState::default();
        let line = |t: &str, pct: u32| {
            format!(
                r#"{{"timestamp":"{t}","type":"event_msg","payload":{{"type":"token_count","info":null,"rate_limits":{{"plan_type":"plus","primary":{{"used_percent":{pct}.4,"window_minutes":300,"resets_at":2000000000}},"secondary":{{"used_percent":36.0,"window_minutes":10080,"resets_at":2000000000}}}}}}}}"#
            )
        };
        s.add_codex_line(&line("2026-11-01T10:00:00.000Z", 12), &mut st, "f");
        s.add_codex_line(&line("2026-11-01T09:00:00.000Z", 99), &mut st, "older");
        let limits = s.limits(parse_time(T).unwrap(), &[], (false, true), None, false);
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].plan.as_deref(), Some("Plus plan"));
        let labels: Vec<_> = limits[0].gauges.iter().map(|g| (g.label.as_str(), g.percent)).collect();
        assert_eq!(labels, vec![("Session", 12), ("Week", 36)]);
    }

    #[test]
    fn shows_a_claude_limit_that_was_hit_until_it_resets() {
        let mut s = Scanner::default();
        s.add_claude_line(
            r#"{"type":"assistant","quotaLimits":{"status":"rejected","rateLimitType":"five_hour","resetsAt":2000}}"#,
        );
        let at = |now| s.limits(now, &[], (true, false), None, false);
        assert_eq!(at(1_000_000)[0].gauges[0].percent, 100);
        assert!(!at(1_000_000)[0].connected);
        assert!(at(3_000_000)[0].gauges.is_empty());
    }

    #[test]
    fn claude_context_comes_from_its_latest_message() {
        let mut s = Scanner::default();
        let line = |id: &str, t: &str, cache: u64, extra: &str| {
            format!(
                r#"{{"timestamp":"{t}","cwd":"/work/Notchify","sessionId":"s1"{extra},"message":{{"id":"{id}","model":"claude-opus-5-5","usage":{{"input_tokens":1,"output_tokens":9,"cache_read_input_tokens":{cache},"cache_creation_input_tokens":0}}}}}}"#
            )
        };
        s.add_claude_line(&line("m1", "2026-11-01T10:00:00.000Z", 100_000, ""));
        s.add_claude_line(&line("m2", "2026-11-01T10:01:00.000Z", 756_174, ""));
        s.add_claude_line(&line("m3", "2026-11-01T10:02:00.000Z", 5, r#","isSidechain":true"#));
        let now = parse_time("2026-11-01T10:02:30.000Z").unwrap();
        let live = s.running(now);
        assert_eq!(live[0].context_percent, Some(76), "the sub-agent's small context doesn't count");
        let limits = s.limits(now, &live, (true, false), None, false);
        assert_eq!(limits[0].context_percent, Some(76));
        assert!(limits[0].running);
    }

    #[test]
    fn context_windows_are_inferred() {
        assert_eq!(claude_window("claude-opus-5-5", 10), 1_000_000);
        assert_eq!(claude_window("claude-sonnet-5-5", 10), 200_000);
        assert_eq!(claude_window("claude-sonnet-5-5", 300_000), 1_000_000);
    }

    #[test]
    fn idle_gaps_are_not_active_time() {
        let min = 60_000;
        assert_eq!(active_minutes(&[]), 0);
        assert_eq!(active_minutes(&[0, 5 * min, 10 * min]), 10);
        // A three-day pause between messages adds nothing.
        assert_eq!(active_minutes(&[0, 5 * min, 4320 * min, 4322 * min]), 7);
    }

    #[test]
    fn keeps_no_conversation_text() {
        let mut s = Scanner::default();
        s.add_claude_line(&claude("msg_1", 2, "a"));
        let kept = format!("{:?}", s.summarize(0));
        assert!(!kept.contains("private words"));
    }
}
