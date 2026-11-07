//! One-click connection of coding agents.
//!
//! Agents that run hooks (Claude Code), a notify program (Codex CLI) or
//! plugins (OpenCode) need a small piece of configuration to report finished
//! turns. [`connect`] writes it, [`disconnect`] removes exactly what it wrote
//! and [`status`] says whether it is in place. Nothing here talks to an
//! account: an agent is "connected" when it reports to the notch.
//!
//! Files are edited in place (JSON via `serde_json`, TOML as text so comments
//! survive), only entries that mention NotchForge are ever touched, and the
//! first edit of a file keeps a `.notchforge.bak` copy.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// How an agent is wired to the notch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wiring {
    /// `~/.claude/settings.json` hooks: Notification, Stop, SubagentStop.
    ClaudeHooks,
    /// `notify = [...]` in `~/.codex/config.toml`.
    CodexNotify,
    /// `~/.config/opencode/plugin/notchforge.js`.
    OpenCodePlugin,
    /// `~/.gemini/settings.json` hooks (the same shape as Claude Code's).
    GeminiHooks,
    /// `~/.cursor/hooks.json`: the turn ends and file edits (Cursor's agent).
    CursorHooks,
    /// `notifications-command` in `~/.aider.conf.yml`.
    AiderNotify,
}

#[derive(Debug, Clone, Copy)]
pub struct AgentDef {
    /// Source key (`claude`, `chatgpt`, `opencode`).
    pub key: &'static str,
    pub name: &'static str,
    pub wiring: Wiring,
    /// What connecting changes, in one line.
    pub detail: &'static str,
}

pub const AGENTS: &[AgentDef] = &[
    AgentDef {
        key: "claude",
        name: "Claude Code",
        wiring: Wiring::ClaudeHooks,
        detail: "Adds Notification, Stop and SubagentStop hooks to ~/.claude/settings.json",
    },
    AgentDef {
        key: "chatgpt",
        name: "Codex CLI",
        wiring: Wiring::CodexNotify,
        detail: "Sets notify in ~/.codex/config.toml",
    },
    AgentDef {
        key: "opencode",
        name: "OpenCode",
        wiring: Wiring::OpenCodePlugin,
        detail: "Installs a plugin in ~/.config/opencode/plugin",
    },
    AgentDef {
        key: "gemini",
        name: "Gemini CLI",
        wiring: Wiring::GeminiHooks,
        detail: "Adds session, tool and turn hooks to ~/.gemini/settings.json",
    },
    AgentDef {
        key: "cursor",
        name: "Cursor Agent",
        wiring: Wiring::CursorHooks,
        detail: "Adds stop and file-edit hooks to ~/.cursor/hooks.json",
    },
    AgentDef {
        key: "aider",
        name: "Aider",
        wiring: Wiring::AiderNotify,
        detail: "Sets notifications-command in ~/.aider.conf.yml",
    },
];

/// Gemini CLI events the notch follows (its names; the webhook maps them).
const GEMINI_EVENTS: [&str; 7] =
    ["SessionStart", "BeforeAgent", "BeforeTool", "AfterTool", "AfterAgent", "Notification", "SessionEnd"];
/// Cursor's hooks that need no reply (the others must answer, so they stay out).
const CURSOR_EVENTS: [&str; 2] = ["stop", "afterFileEdit"];
const AIDER_BEGIN: &str = "# >>> notchforge (Notchisland) >>>";
const AIDER_END: &str = "# <<< notchforge (Notchisland) <<<";

fn gemini_settings(env: &Env) -> PathBuf {
    env.home.join(".gemini/settings.json")
}
fn cursor_hooks(env: &Env) -> PathBuf {
    env.home.join(".cursor/hooks.json")
}
fn aider_config(env: &Env) -> PathBuf {
    env.home.join(".aider.conf.yml")
}

/// A fire-and-forget hook command posting the event to `/v1/hooks`.
fn hook_post(env: &Env, source: &str) -> String {
    let auth = env.token.as_deref().map(|t| format!(" -H 'Authorization: Bearer {t}'")).unwrap_or_default();
    format!(
        "curl -s -m 3 -o /dev/null -X POST 'http://127.0.0.1:{}/v1/hooks?source={source}' -H 'Content-Type: application/json'{auth} --data-binary @- || true",
        env.port
    )
}

fn aider_block(env: &Env) -> String {
    let auth = env.token.as_deref().map(|t| format!(" -H \"Authorization: Bearer {t}\"")).unwrap_or_default();
    format!(
        "{AIDER_BEGIN}\nnotifications: true\nnotifications-command: 'curl -s -m 3 -o /dev/null -X POST \"http://127.0.0.1:{}/v1/notify?source=aider&kind=task_finished\" -H \"Content-Type: application/json\"{auth} --data \"{{}}\"'\n{AIDER_END}\n",
        env.port
    )
}

/// Does `text` (Aider's YAML) set a notifications command of its own?
fn aider_foreign_command(text: &str) -> bool {
    let mut inside = false;
    for l in text.lines() {
        if l.trim() == AIDER_BEGIN {
            inside = true;
        } else if l.trim() == AIDER_END {
            inside = false;
        } else if !inside && l.trim_start().starts_with("notifications-command") {
            return true;
        }
    }
    false
}

fn strip_aider_block(text: &str) -> String {
    let mut out = Vec::new();
    let mut inside = false;
    for l in text.lines() {
        if l.trim() == AIDER_BEGIN {
            inside = true;
        } else if l.trim() == AIDER_END {
            inside = false;
        } else if !inside {
            out.push(l);
        }
    }
    let mut s = out.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// Add our hook to every `events` list in a Claude-shaped `hooks` object
/// (Gemini CLI uses the same shape), replacing an older one of ours.
fn merge_hooks(v: &mut Value, events: &[&str], group: impl Fn(&str) -> Value) -> Result<(), String> {
    if !v.is_object() {
        return Err("settings file is not a JSON object".into());
    }
    let hooks = v.as_object_mut().expect("object").entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("\"hooks\" is not an object")?;
    for event in events {
        let groups = hooks.entry(*event).or_insert_with(|| json!([]));
        let groups = groups.as_array_mut().ok_or(format!("hooks.{event} is not a list"))?;
        groups.retain(|g| !is_ours(g) && !is_ours_flat(g));
        groups.push(group(event));
    }
    Ok(())
}

/// Remove our hooks from a `hooks` object, dropping lists left empty.
fn unmerge_hooks(v: &mut Value, events: &[&str]) {
    if let Some(hooks) = v.get_mut("hooks").and_then(Value::as_object_mut) {
        for event in events {
            if let Some(groups) = hooks.get_mut(*event).and_then(Value::as_array_mut) {
                groups.retain(|g| !is_ours(g) && !is_ours_flat(g));
            }
            if hooks.get(*event).and_then(Value::as_array).is_some_and(Vec::is_empty) {
                hooks.remove(*event);
            }
        }
        if hooks.is_empty() {
            v.as_object_mut().expect("object").remove("hooks");
        }
    }
}

/// Cursor's hook entries are flat: `{ "command": … }`.
fn is_ours_flat(h: &Value) -> bool {
    h["command"].as_str().is_some_and(|c| c.contains("/v1/hooks") || c.contains(MARKER))
}

fn hooks_connected(v: &Value, events: &[&str]) -> bool {
    events.iter().all(|e| v["hooks"][*e].as_array().is_some_and(|g| g.iter().any(|g| is_ours(g) || is_ours_flat(g))))
}

/// Where things are, injectable for tests.
#[derive(Debug, Clone)]
pub struct Env {
    pub home: PathBuf,
    /// The `notchforge-notify` CLI shipped next to the app, if present.
    pub notify_bin: Option<PathBuf>,
    pub port: u16,
    pub token: Option<String>,
}

impl Env {
    /// The real home directory and the CLI beside the running executable.
    pub fn current(port: u16, token: Option<String>) -> Env {
        let notify_bin = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join("notchforge-notify")))
            .filter(|p| p.exists());
        Env { home: dirs::home_dir().unwrap_or_default(), notify_bin, port, token }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Connected,
    NotConnected,
    /// The agent's config can't be edited safely (message explains).
    Blocked(String),
}

const MARKER: &str = "notchforge";
const PLUGIN_JS: &str = include_str!("../../../../integrations/opencode/notchforge.js");
/// Every Claude Code event the notch follows: the live sessions update on
/// each one, `PermissionRequest` is answered from the notch, and
/// Notification / Stop / SubagentStop still become notifications.
const CLAUDE_EVENTS: [&str; 9] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Notification",
    "Stop",
    "SubagentStop",
    "SessionEnd",
];
/// Events whose hooks take a tool matcher.
const CLAUDE_TOOL_EVENTS: [&str; 3] = ["PreToolUse", "PostToolUse", "PermissionRequest"];

fn claude_settings(env: &Env) -> PathBuf {
    env.home.join(".claude/settings.json")
}
fn codex_config(env: &Env) -> PathBuf {
    env.home.join(".codex/config.toml")
}
fn opencode_plugin(env: &Env) -> PathBuf {
    env.home.join(".config/opencode/plugin/notchforge.js")
}

fn shell_quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c)) {
        s.into_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Where Claude Code's status line reports limits: the file it writes and the
/// script that writes it, both in the app's data folder.
pub const CLAUDE_STATUS_FILE: &str = "claude-status.json";
const CLAUDE_STATUS_SCRIPT: &str = "notchforge-claude-status.sh";
const CLAUDE_STATUS_PREVIOUS: &str = "notchforge-claude-status.previous.json";

fn status_line_is_ours(v: &Value) -> bool {
    v["statusLine"]["command"].as_str().is_some_and(|c| c.contains(CLAUDE_STATUS_SCRIPT))
}

/// Is Claude Code's status line reporting limits to the notch?
pub fn claude_limits_status(env: &Env) -> Status {
    match read_json(&claude_settings(env)) {
        Ok(v) if status_line_is_ours(&v) => Status::Connected,
        Ok(_) => Status::NotConnected,
        Err(e) => Status::Blocked(e),
    }
}

/// Point Claude Code's status line at a small script that saves what it is
/// given for the notch. A status line already in place keeps running behind it
/// and comes back on [`disconnect_claude_limits`].
pub fn connect_claude_limits(env: &Env, data_dir: &Path) -> Result<String, String> {
    let path = claude_settings(env);
    let mut v = read_json(&path)?;
    if !v.is_object() {
        return Err(format!("{} is not a JSON object", path.display()));
    }
    std::fs::create_dir_all(data_dir).map_err(|e| format!("cannot create {}: {e}", data_dir.display()))?;
    let script = data_dir.join(CLAUDE_STATUS_SCRIPT);
    let previous_file = data_dir.join(CLAUDE_STATUS_PREVIOUS);
    if !status_line_is_ours(&v) {
        match v.get("statusLine") {
            Some(old) => std::fs::write(&previous_file, old.to_string())
                .map_err(|e| format!("cannot write {}: {e}", previous_file.display()))?,
            None => {
                let _ = std::fs::remove_file(&previous_file);
            }
        }
    }
    let previous = read_json(&previous_file).unwrap_or_default();
    let chained = previous["command"].as_str().map_or(String::new(), |c| format!("printf '%s' \"$input\" | {c}\n"));
    let body = format!(
        "#!/bin/sh\n# Written by Notchisland: saves Claude Code's status data so the notch can show its limits.\ninput=$(cat)\ndir={}\ntmp=\"$dir/.claude-status.$$\"\n(umask 077; printf '%s' \"$input\" > \"$tmp\") && mv -f \"$tmp\" \"$dir/{CLAUDE_STATUS_FILE}\"\n{chained}",
        shell_quote(data_dir)
    );
    std::fs::write(&script, body).map_err(|e| format!("cannot write {}: {e}", script.display()))?;
    v.as_object_mut().expect("object").insert(
        "statusLine".into(),
        json!({ "type": "command", "command": format!("/bin/sh {}", shell_quote(&script)) }),
    );
    write_text(&path, &(serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n"))?;
    Ok("Limits appear the next time Claude Code responds in a terminal. A custom status line hides some of its footer hints, such as \u{201c}esc to interrupt\u{201d}. Disconnect in Settings puts things back.".into())
}

/// Undo [`connect_claude_limits`], restoring any earlier status line.
pub fn disconnect_claude_limits(env: &Env, data_dir: &Path) -> Result<(), String> {
    let path = claude_settings(env);
    let mut v = read_json(&path)?;
    if status_line_is_ours(&v) {
        let previous_file = data_dir.join(CLAUDE_STATUS_PREVIOUS);
        let o = v.as_object_mut().expect("object");
        match std::fs::read_to_string(&previous_file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            Some(old) => {
                o.insert("statusLine".into(), old);
            }
            None => {
                o.remove("statusLine");
            }
        }
        write_text(&path, &(serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n"))?;
        let _ = std::fs::remove_file(previous_file);
    }
    let _ = std::fs::remove_file(data_dir.join(CLAUDE_STATUS_SCRIPT));
    let _ = std::fs::remove_file(data_dir.join(CLAUDE_STATUS_FILE));
    Ok(())
}

/// The hook command for one Claude Code event: the event's JSON goes to the
/// notch's `/v1/hooks`. Fails silently when the app isn't running. Only the
/// permission request prints the reply (Claude Code reads it as the
/// decision); it may wait while the notch asks.
fn claude_command(env: &Env, event: &str) -> String {
    let auth = env.token.as_deref().map(|t| format!(" -H 'Authorization: Bearer {t}'")).unwrap_or_default();
    let (max, out) = if event == "PermissionRequest" { (590, "") } else { (3, " -o /dev/null") };
    format!(
        "curl -s -m {max}{out} -X POST 'http://127.0.0.1:{}/v1/hooks?source=claude' -H 'Content-Type: application/json'{auth} --data-binary @- || true",
        env.port,
    )
}

/// One hook group as written into settings.json.
fn claude_group(env: &Env, event: &str) -> Value {
    let mut hook = json!({ "type": "command", "command": claude_command(env, event) });
    if event != "PermissionRequest" {
        hook["timeout"] = json!(10);
    }
    let mut group = json!({ "hooks": [hook] });
    if CLAUDE_TOOL_EVENTS.contains(&event) {
        group["matcher"] = json!("*");
    }
    group
}

fn backup_once(path: &Path) {
    let bak = path.with_extension(match path.extension() {
        Some(e) => format!("{}.notchforge.bak", e.to_string_lossy()),
        None => "notchforge.bak".into(),
    });
    if path.exists() && !bak.exists() {
        let _ = std::fs::copy(path, bak);
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(json!({})),
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("{} is not valid JSON ({e})", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    backup_once(path);
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn is_ours(group: &Value) -> bool {
    group["hooks"].as_array().is_some_and(|hs| {
        hs.iter().any(|h| {
            h["command"]
                .as_str()
                .is_some_and(|c| c.contains(MARKER) || c.contains("/v1/notify") || c.contains("/v1/hooks"))
        })
    })
}

/// Find the top-level `notify` line of a TOML file: (line index, text).
fn codex_notify_line(text: &str) -> Option<(usize, &str)> {
    for (i, l) in text.lines().enumerate() {
        let t = l.trim_start();
        if t.starts_with('[') {
            return None; // tables begin: top-level keys are over
        }
        if t.starts_with("notify") && t[6..].trim_start().starts_with('=') {
            return Some((i, l));
        }
    }
    None
}

pub fn status(def: &AgentDef, env: &Env) -> Status {
    match def.wiring {
        Wiring::ClaudeHooks => match read_json(&claude_settings(env)) {
            Ok(v) => {
                let all =
                    CLAUDE_EVENTS.iter().all(|e| v["hooks"][*e].as_array().is_some_and(|g| g.iter().any(is_ours)));
                if all { Status::Connected } else { Status::NotConnected }
            }
            Err(e) => Status::Blocked(e),
        },
        Wiring::CodexNotify => {
            let text = std::fs::read_to_string(codex_config(env)).unwrap_or_default();
            match codex_notify_line(&text) {
                Some((_, l)) if l.contains(MARKER) => Status::Connected,
                Some(_) => Status::Blocked("Codex already runs another notify program".into()),
                None => Status::NotConnected,
            }
        }
        Wiring::OpenCodePlugin => {
            if opencode_plugin(env).exists() {
                Status::Connected
            } else {
                Status::NotConnected
            }
        }
        Wiring::GeminiHooks => match read_json(&gemini_settings(env)) {
            Ok(v) if hooks_connected(&v, &GEMINI_EVENTS) => Status::Connected,
            Ok(_) => Status::NotConnected,
            Err(e) => Status::Blocked(e),
        },
        Wiring::CursorHooks => match read_json(&cursor_hooks(env)) {
            Ok(v) if hooks_connected(&v, &CURSOR_EVENTS) => Status::Connected,
            Ok(_) => Status::NotConnected,
            Err(e) => Status::Blocked(e),
        },
        Wiring::AiderNotify => {
            let text = std::fs::read_to_string(aider_config(env)).unwrap_or_default();
            if text.contains(AIDER_BEGIN) {
                Status::Connected
            } else if aider_foreign_command(&text) {
                Status::Blocked("Aider already runs another notifications command".into())
            } else {
                Status::NotConnected
            }
        }
    }
}

/// Wire the agent to the notch. Returns a short note for the UI.
pub fn connect(def: &AgentDef, env: &Env) -> Result<String, String> {
    match def.wiring {
        Wiring::ClaudeHooks => {
            let path = claude_settings(env);
            let mut v = read_json(&path)?;
            if !v.is_object() {
                return Err(format!("{} is not a JSON object", path.display()));
            }
            let hooks = v.as_object_mut().expect("object").entry("hooks").or_insert_with(|| json!({}));
            let hooks = hooks.as_object_mut().ok_or("\"hooks\" in settings.json is not an object")?;
            for event in CLAUDE_EVENTS {
                let groups = hooks.entry(event).or_insert_with(|| json!([]));
                let groups = groups.as_array_mut().ok_or(format!("hooks.{event} is not a list"))?;
                groups.retain(|g| !is_ours(g));
                groups.push(claude_group(env, event));
            }
            write_text(&path, &(serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n"))?;
            Ok("Claude Code now reports to the notch, and asks there before running a tool. Restart running sessions to pick it up.".into())
        }
        Wiring::CodexNotify => {
            let bin = env.notify_bin.as_ref().ok_or("notchforge-notify was not found next to the app")?;
            let path = codex_config(env);
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let line = format!(
                "notify = [{}, \"--source\", \"chatgpt\", \"--best-effort\"]",
                serde_json::to_string(&bin.to_string_lossy()).map_err(|e| e.to_string())?
            );
            let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
            match codex_notify_line(&text) {
                Some((_, l)) if !l.contains(MARKER) => {
                    return Err(
                        "Codex already runs another notify program; remove it from ~/.codex/config.toml first".into()
                    );
                }
                Some((i, _)) => lines[i] = line,
                None => {
                    let at = lines.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(lines.len());
                    lines.insert(at, line);
                    if at < lines.len() - 1 {
                        lines.insert(at + 1, String::new());
                    }
                }
            }
            write_text(&path, &(lines.join("\n") + "\n"))?;
            Ok("Codex now reports each finished turn to the notch.".into())
        }
        Wiring::OpenCodePlugin => {
            write_text(&opencode_plugin(env), PLUGIN_JS)?;
            Ok("OpenCode loads the plugin the next time it starts.".into())
        }
        Wiring::GeminiHooks => {
            let path = gemini_settings(env);
            let mut v = read_json(&path)?;
            merge_hooks(&mut v, &GEMINI_EVENTS, |e| {
                let mut g = json!({ "hooks": [{ "type": "command", "command": hook_post(env, "gemini") }] });
                if e == "BeforeTool" || e == "AfterTool" {
                    g["matcher"] = json!("*");
                }
                g
            })?;
            write_text(&path, &(serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n"))?;
            Ok("Gemini CLI now reports to the notch. Restart running sessions to pick it up.".into())
        }
        Wiring::CursorHooks => {
            let path = cursor_hooks(env);
            let mut v = read_json(&path)?;
            if v.get("version").is_none() {
                v["version"] = json!(1);
            }
            merge_hooks(&mut v, &CURSOR_EVENTS, |_| json!({ "command": hook_post(env, "cursor") }))?;
            write_text(&path, &(serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n"))?;
            Ok("Cursor's agent now reports finished turns and edits to the notch.".into())
        }
        Wiring::AiderNotify => {
            let path = aider_config(env);
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            if aider_foreign_command(&text) {
                return Err(
                    "Aider already runs another notifications command; remove it from ~/.aider.conf.yml first".into()
                );
            }
            let mut out = strip_aider_block(&text);
            out.push_str(&aider_block(env));
            write_text(&path, &out)?;
            Ok("Aider now tells the notch when it is waiting for you.".into())
        }
    }
}

/// Bring an older Claude Code connection (fewer hooks, or the notify CLI)
/// up to the current set, without asking: it was connected on purpose.
/// Returns whether anything changed.
pub fn upgrade_claude(env: &Env) -> bool {
    let Ok(v) = read_json(&claude_settings(env)) else { return false };
    let ours = |e: &str| v["hooks"][e].as_array().is_some_and(|g| g.iter().any(is_ours));
    let any = CLAUDE_EVENTS.iter().any(|e| ours(e));
    let current = CLAUDE_EVENTS
        .iter()
        .all(|e| v["hooks"][*e].as_array().is_some_and(|g| g.iter().any(|g| *g == claude_group(env, e))));
    if !any || current {
        return false;
    }
    connect(&AGENTS[0], env).is_ok()
}

/// Remove what [`connect`] added, leaving everything else as it was.
pub fn disconnect(def: &AgentDef, env: &Env) -> Result<(), String> {
    match def.wiring {
        Wiring::ClaudeHooks => {
            let path = claude_settings(env);
            let mut v = read_json(&path)?;
            if let Some(hooks) = v.get_mut("hooks").and_then(Value::as_object_mut) {
                for event in CLAUDE_EVENTS {
                    if let Some(groups) = hooks.get_mut(event).and_then(Value::as_array_mut) {
                        groups.retain(|g| !is_ours(g));
                    }
                    if hooks.get(event).and_then(Value::as_array).is_some_and(Vec::is_empty) {
                        hooks.remove(event);
                    }
                }
                if hooks.is_empty() {
                    v.as_object_mut().expect("object").remove("hooks");
                }
            }
            write_text(&path, &(serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n"))
        }
        Wiring::CodexNotify => {
            let path = codex_config(env);
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            if let Some((i, l)) = codex_notify_line(&text) {
                if l.contains(MARKER) {
                    let kept: Vec<&str> = text.lines().enumerate().filter(|(n, _)| *n != i).map(|(_, l)| l).collect();
                    write_text(&path, &(kept.join("\n") + "\n"))?;
                }
            }
            Ok(())
        }
        Wiring::OpenCodePlugin => match std::fs::remove_file(opencode_plugin(env)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        },
        Wiring::GeminiHooks | Wiring::CursorHooks => {
            let (path, events): (PathBuf, &[&str]) = if def.wiring == Wiring::GeminiHooks {
                (gemini_settings(env), &GEMINI_EVENTS)
            } else {
                (cursor_hooks(env), &CURSOR_EVENTS)
            };
            let mut v = read_json(&path)?;
            unmerge_hooks(&mut v, events);
            write_text(&path, &(serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n"))
        }
        Wiring::AiderNotify => {
            let path = aider_config(env);
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            if text.contains(AIDER_BEGIN) {
                write_text(&path, &strip_aider_block(&text))?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(tag: &str, bin: bool) -> Env {
        let home = std::env::temp_dir().join(format!("nf-connect-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("home");
        Env {
            home,
            notify_bin: bin.then(|| PathBuf::from("/Applications/Notchisland.app/Contents/MacOS/notchforge-notify")),
            port: 47815,
            token: None,
        }
    }

    #[test]
    fn claude_hooks_merge_and_unmerge_without_touching_other_settings() {
        let e = env("claude", true);
        let path = claude_settings(&e);
        std::fs::create_dir_all(path.parent().expect("dir")).expect("dir");
        std::fs::write(
            &path,
            r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}]}}"#,
        )
        .expect("write");
        let def = &AGENTS[0];
        assert_eq!(status(def, &e), Status::NotConnected);
        connect(def, &e).expect("connect");
        connect(def, &e).expect("connect again is idempotent");
        assert_eq!(status(def, &e), Status::Connected);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(v["model"], "opus");
        assert_eq!(v["hooks"]["Stop"].as_array().expect("stop").len(), 2, "user's hook kept, ours added once");
        let cmd = |e: &str| v["hooks"][e][0]["hooks"][0]["command"].as_str().unwrap_or("").to_owned();
        assert!(cmd("SubagentStop").contains("/v1/hooks?source=claude") && cmd("SubagentStop").contains("/dev/null"));
        assert!(!cmd("PermissionRequest").contains("/dev/null"), "the decision is printed for Claude Code");
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "*");
        assert!(path.with_extension("json.notchforge.bak").exists());
        assert!(!upgrade_claude(&e), "already current");
        disconnect(def, &e).expect("disconnect");
        assert!(!upgrade_claude(&e), "never connects what was disconnected");
        assert_eq!(status(def, &e), Status::NotConnected);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(v["hooks"]["Stop"].as_array().expect("stop").len(), 1);
        assert!(v["hooks"].get("Notification").is_none());
        let _ = std::fs::remove_dir_all(&e.home);
    }

    #[test]
    fn claude_hooks_carry_the_token() {
        let mut e = env("curl", false);
        e.token = Some("s3cret".into());
        connect(&AGENTS[0], &e).expect("connect");
        let text = std::fs::read_to_string(claude_settings(&e)).expect("read");
        assert!(text.contains("curl -s -m 3") && text.contains("Bearer s3cret") && text.contains("/v1/hooks"));
        assert_eq!(status(&AGENTS[0], &e), Status::Connected);
        let _ = std::fs::remove_dir_all(&e.home);
    }

    #[test]
    fn gemini_cursor_and_aider_connect_and_disconnect_cleanly() {
        let e = env("more", false);
        let def = |k: &str| *AGENTS.iter().find(|d| d.key == k).expect("agent");
        std::fs::create_dir_all(e.home.join(".gemini")).expect("dir");
        std::fs::write(gemini_settings(&e), r#"{"ui":{"theme":"dark"}}"#).expect("write");
        for k in ["gemini", "cursor", "aider"] {
            assert_eq!(status(&def(k), &e), Status::NotConnected, "{k}");
            connect(&def(k), &e).expect("connect");
            connect(&def(k), &e).expect("idempotent");
            assert_eq!(status(&def(k), &e), Status::Connected, "{k}");
        }
        let g: Value =
            serde_json::from_str(&std::fs::read_to_string(gemini_settings(&e)).expect("read")).expect("json");
        assert_eq!(g["ui"]["theme"], "dark");
        assert_eq!(g["hooks"]["BeforeTool"].as_array().expect("list").len(), 1);
        assert!(g["hooks"]["AfterAgent"][0]["hooks"][0]["command"].as_str().expect("cmd").contains("source=gemini"));
        let c: Value = serde_json::from_str(&std::fs::read_to_string(cursor_hooks(&e)).expect("read")).expect("json");
        assert_eq!(c["version"], 1);
        assert!(c["hooks"]["stop"][0]["command"].as_str().expect("cmd").contains("source=cursor"));
        let a = std::fs::read_to_string(aider_config(&e)).expect("read");
        assert_eq!(a.matches(AIDER_BEGIN).count(), 1);
        for k in ["gemini", "cursor", "aider"] {
            disconnect(&def(k), &e).expect("disconnect");
            assert_eq!(status(&def(k), &e), Status::NotConnected, "{k}");
        }
        let g: Value =
            serde_json::from_str(&std::fs::read_to_string(gemini_settings(&e)).expect("read")).expect("json");
        assert!(g.get("hooks").is_none());
        std::fs::write(aider_config(&e), "notifications-command: say hi\n").expect("write");
        assert!(matches!(status(&def("aider"), &e), Status::Blocked(_)));
        assert!(connect(&def("aider"), &e).is_err());
        let _ = std::fs::remove_dir_all(&e.home);
    }

    #[test]
    fn invalid_json_is_never_overwritten() {
        let e = env("bad", true);
        let path = claude_settings(&e);
        std::fs::create_dir_all(path.parent().expect("dir")).expect("dir");
        std::fs::write(&path, "{ not json").expect("write");
        assert!(connect(&AGENTS[0], &e).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{ not json");
        assert!(matches!(status(&AGENTS[0], &e), Status::Blocked(_)));
        let _ = std::fs::remove_dir_all(&e.home);
    }

    #[test]
    fn codex_notify_is_inserted_before_tables_and_refuses_foreign_programs() {
        let e = env("codex", true);
        let path = codex_config(&e);
        std::fs::create_dir_all(path.parent().expect("dir")).expect("dir");
        std::fs::write(&path, "model = \"gpt-5\"\n# keep me\n[projects]\nx = 1\n").expect("write");
        let def = &AGENTS[1];
        connect(def, &e).expect("connect");
        let text = std::fs::read_to_string(&path).expect("read");
        let notify_at = text.find("notify =").expect("notify");
        assert!(notify_at < text.find("[projects]").expect("table"), "top-level key precedes tables");
        assert!(text.contains("# keep me"));
        assert_eq!(status(def, &e), Status::Connected);
        disconnect(def, &e).expect("disconnect");
        assert!(!std::fs::read_to_string(&path).expect("read").contains("notify"));

        std::fs::write(&path, "notify = [\"my-tool\"]\n").expect("write");
        assert!(matches!(status(def, &e), Status::Blocked(_)));
        assert!(connect(def, &e).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "notify = [\"my-tool\"]\n");
        let _ = std::fs::remove_dir_all(&e.home);
    }

    #[test]
    fn claude_limits_keep_and_restore_an_existing_status_line() {
        let e = env("limits", true);
        let data = e.home.join("data dir");
        let path = claude_settings(&e);
        std::fs::create_dir_all(path.parent().expect("dir")).expect("dir");
        std::fs::write(&path, r#"{"model":"opus","statusLine":{"type":"command","command":"my-line","padding":2}}"#)
            .expect("write");
        assert_eq!(claude_limits_status(&e), Status::NotConnected);
        connect_claude_limits(&e, &data).expect("connect");
        connect_claude_limits(&e, &data).expect("connect again keeps the original");
        assert_eq!(claude_limits_status(&e), Status::Connected);
        let script = std::fs::read_to_string(data.join(CLAUDE_STATUS_SCRIPT)).expect("script");
        assert!(script.contains("| my-line"), "the earlier status line still runs");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(v["model"], "opus");
        disconnect_claude_limits(&e, &data).expect("disconnect");
        assert_eq!(claude_limits_status(&e), Status::NotConnected);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(v["statusLine"]["command"], "my-line");
        assert_eq!(v["statusLine"]["padding"], 2);
        assert!(!data.join(CLAUDE_STATUS_SCRIPT).exists());
        let _ = std::fs::remove_dir_all(&e.home);
    }

    #[test]
    fn claude_limits_script_saves_what_it_is_given() {
        let e = env("limits-run", true);
        let data = e.home.join("data");
        connect_claude_limits(&e, &data).expect("connect");
        let mut child = std::process::Command::new("/bin/sh")
            .arg(data.join(CLAUDE_STATUS_SCRIPT))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn");
        use std::io::Write;
        child.stdin.take().expect("stdin").write_all(b"{\"session_id\":\"s\"}").expect("write");
        assert!(child.wait().expect("wait").success());
        assert_eq!(std::fs::read_to_string(data.join(CLAUDE_STATUS_FILE)).expect("saved"), "{\"session_id\":\"s\"}");
        disconnect_claude_limits(&e, &data).expect("disconnect");
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(claude_settings(&e)).expect("read")).expect("json");
        assert!(v.get("statusLine").is_none());
        let _ = std::fs::remove_dir_all(&e.home);
    }

    #[test]
    fn opencode_plugin_installs_and_removes() {
        let e = env("opencode", false);
        let def = &AGENTS[2];
        assert_eq!(status(def, &e), Status::NotConnected);
        connect(def, &e).expect("connect");
        assert_eq!(status(def, &e), Status::Connected);
        assert!(std::fs::read_to_string(opencode_plugin(&e)).expect("read").contains("session.idle"));
        disconnect(def, &e).expect("disconnect");
        assert_eq!(status(def, &e), Status::NotConnected);
        let _ = std::fs::remove_dir_all(&e.home);
    }
}
