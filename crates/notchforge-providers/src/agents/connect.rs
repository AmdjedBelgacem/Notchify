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
];

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
const CLAUDE_EVENTS: [&str; 3] = ["Notification", "Stop", "SubagentStop"];

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

/// The hook command for one Claude Code event.
fn claude_command(env: &Env, event: &str) -> String {
    let kind = if event == "SubagentStop" { " --kind task_finished" } else { "" };
    match &env.notify_bin {
        Some(bin) => format!("{} --source claude{kind} --stdin --best-effort", shell_quote(bin)),
        None => {
            let auth = env.token.as_deref().map(|t| format!(" -H 'Authorization: Bearer {t}'")).unwrap_or_default();
            format!(
                "curl -s -m 2 -X POST 'http://127.0.0.1:{}/v1/notify?source=claude{}' -H 'Content-Type: application/json'{auth} --data-binary @- || true",
                env.port,
                if event == "SubagentStop" { "&kind=task_finished" } else { "" },
            )
        }
    }
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
        hs.iter().any(|h| h["command"].as_str().is_some_and(|c| c.contains(MARKER) || c.contains("/v1/notify")))
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
                groups.push(json!({ "hooks": [{ "type": "command", "command": claude_command(env, event) }] }));
            }
            write_text(&path, &(serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n"))?;
            Ok("Claude Code now reports to the notch. Restart running sessions to pick it up.".into())
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
    }
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
            notify_bin: bin.then(|| PathBuf::from("/Applications/NotchForge.app/Contents/MacOS/notchforge-notify")),
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
        assert!(
            v["hooks"]["SubagentStop"][0]["hooks"][0]["command"]
                .as_str()
                .expect("cmd")
                .contains("--kind task_finished")
        );
        assert!(path.with_extension("json.notchforge.bak").exists());
        disconnect(def, &e).expect("disconnect");
        assert_eq!(status(def, &e), Status::NotConnected);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(v["hooks"]["Stop"].as_array().expect("stop").len(), 1);
        assert!(v["hooks"].get("Notification").is_none());
        let _ = std::fs::remove_dir_all(&e.home);
    }

    #[test]
    fn claude_falls_back_to_curl_without_the_cli() {
        let mut e = env("curl", false);
        e.token = Some("s3cret".into());
        connect(&AGENTS[0], &e).expect("connect");
        let text = std::fs::read_to_string(claude_settings(&e)).expect("read");
        assert!(text.contains("curl -s -m 2") && text.contains("Bearer s3cret") && text.contains("/v1/notify"));
        assert_eq!(status(&AGENTS[0], &e), Status::Connected);
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
