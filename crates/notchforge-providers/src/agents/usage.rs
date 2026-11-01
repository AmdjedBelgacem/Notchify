//! Token usage of coding agents, read from the session logs they already keep:
//! Claude Code in `~/.claude/projects`, Codex CLI in `~/.codex/sessions`.
//!
//! Only token counts, model names, times and the project folder's name are
//! kept. The logs also hold whole conversations, which are never stored or
//! sent anywhere. Files are read once and then only from where the last read
//! stopped, because the logs can run to gigabytes.

use notchforge_core::model::{
    AgentUsage, ProviderStatus, RunningSession, TokenCounts, UsageAgent, UsageRange, UsageSession,
};
use notchforge_core::provider::{
    BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent, Wake,
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
/// A session that logged tokens this recently counts as running.
const RUNNING_MS: i64 = 180_000;
const TOP_SESSIONS: usize = 4;

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

fn project_name(cwd: &str) -> String {
    Path::new(cwd).file_name().and_then(|n| n.to_str()).unwrap_or(cwd).to_owned()
}

#[derive(Deserialize)]
struct ClaudeLine {
    timestamp: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    message: Option<ClaudeMessage>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ClaudeUsage>,
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
    first_ms: i64,
    last_ms: i64,
    /// (tokens in use, window), where the agent logs it.
    context: Option<(u64, u64)>,
}

#[derive(Default)]
struct FileState {
    offset: u64,
    /// Codex states these once, ahead of the token counts that use them.
    model: Option<Arc<str>>,
    session: Option<Arc<str>>,
}

/// Everything read from the logs so far.
#[derive(Default)]
pub struct Scanner {
    files: HashMap<PathBuf, FileState>,
    records: HashMap<String, Record>,
    sessions: HashMap<Arc<str>, SessionInfo>,
    strings: HashMap<String, Arc<str>>,
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
        let info = self.sessions.entry(key.clone()).or_insert_with(|| SessionInfo {
            first_ms: ts_ms,
            last_ms: ts_ms,
            ..Default::default()
        });
        info.first_ms = info.first_ms.min(ts_ms);
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
        if !line.contains("\"usage\"") {
            return;
        }
        let Ok(l) = serde_json::from_str::<ClaudeLine>(line) else { return };
        let Some(msg) = l.message else { return };
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

    /// One Codex CLI log line. Its `input_tokens` already include cached ones.
    fn add_codex_line(&mut self, line: &str, state: &mut FileState, fallback: &str) {
        if !(line.contains("token_count") || line.contains("session_meta") || line.contains("turn_context")) {
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
            Some("event_msg") if payload.get("type").and_then(|v| v.as_str()) == Some("token_count") => {
                let Some(ts) = ts else { return };
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
        self.records.retain(|_, r| r.ts_ms >= cutoff);
        self.sessions.retain(|_, s| s.last_ms >= cutoff);
    }

    /// The three ranges the page shows, ending today.
    pub fn summarize(&self, today_start_ms: i64) -> Vec<UsageRange> {
        let hours = self.range("Today", today_start_ms, 24, HOUR_MS);
        let week = self.range("7 Days", today_start_ms - 6 * DAY_MS, 7, DAY_MS);
        let month = self.range("30 Days", today_start_ms - 29 * DAY_MS, 30, DAY_MS);
        vec![hours, week, month]
    }

    fn range(&self, label: &str, start_ms: i64, buckets: usize, step_ms: i64) -> UsageRange {
        let mut out = UsageRange { label: label.to_owned(), bars: vec![(0, 0); buckets], ..Default::default() };
        let mut agents: HashMap<UsageAgent, u64> = HashMap::new();
        let mut models: HashMap<&str, u64> = HashMap::new();
        // Per session: tokens, and tokens per model to name its main one.
        let mut sessions: HashMap<&Arc<str>, SessionTally> = HashMap::new();
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
            match r.agent {
                UsageAgent::ClaudeCode => out.bars[i].0 += tokens,
                UsageAgent::Codex => out.bars[i].1 += tokens,
            }
            *agents.entry(r.agent).or_default() += tokens;
            *models.entry(&r.model).or_default() += tokens;
            let s = sessions.entry(&r.session).or_insert((0, r.agent, HashMap::new()));
            s.0 += tokens;
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
                    minutes: info.map_or(0, |i| ((i.last_ms - i.first_ms) / 60_000) as u32),
                    tokens,
                }
            })
            .collect();
        list.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.project.cmp(&b.project)));
        list.truncate(TOP_SESSIONS);
        out.sessions = list;
        out
    }

    /// Sessions that logged tokens in the last few minutes, latest first.
    pub fn running(&self, now_ms: i64) -> Vec<RunningSession> {
        let mut tokens: HashMap<&Arc<str>, (u64, UsageAgent)> = HashMap::new();
        for r in self.records.values() {
            let e = tokens.entry(&r.session).or_insert((0, r.agent));
            e.0 += r.counts.main();
        }
        let mut live: Vec<(i64, RunningSession)> = self
            .sessions
            .iter()
            .filter(|(_, s)| now_ms - s.last_ms <= RUNNING_MS)
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

fn read_usage(scanner: &mut Scanner, home: &Path, now_ms: i64) -> AgentUsage {
    scanner.scan(home, now_ms);
    let today = local_day_start(now_ms);
    let mut ranges = scanner.summarize(today);
    add_ticks(&mut ranges, today);
    AgentUsage {
        loaded: true,
        claude_found: home.join(".claude/projects").is_dir(),
        codex_found: home.join(".codex/sessions").is_dir(),
        opencode_found: home.join(".local/share/opencode").is_dir(),
        ranges,
        running: scanner.running(now_ms),
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
                // The first read can cover gigabytes: keep it off the runtime.
                let Ok((back, usage)) = tokio::task::spawn_blocking(move || {
                    let usage = read_usage(&mut scanner, &h, now);
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
                if ctx.sleep(POLL).await == Wake::Shutdown {
                    return Ok(());
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "2026-11-01T10:30:00.500Z";

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
        assert_eq!(r.bars[10], (68, 0), "cache stays out of the bars");
        assert_eq!(r.agents, vec![(UsageAgent::ClaudeCode, 68)]);
        assert_eq!(r.models, vec![("Opus 5.5".to_owned(), 68)]);
        assert_eq!(r.sessions[0].project, "Notchify");
        assert_eq!(r.sessions[0].tokens, 68);
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
        assert!(s.running(now + RUNNING_MS + 1).is_empty());
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
    fn keeps_no_conversation_text() {
        let mut s = Scanner::default();
        s.add_claude_line(&claude("msg_1", 2, "a"));
        let kept = format!("{:?}", s.summarize(0));
        assert!(!kept.contains("private words"));
    }
}
