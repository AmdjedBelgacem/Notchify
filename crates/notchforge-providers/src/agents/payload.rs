//! The coding-agent event schema and its mapping to [`NewNotification`].
//!
//! The schema is deliberately forgiving: agents and hook scripts send whatever
//! they have, and field aliases cover the common spellings. A minimal event is
//!
//! ```json
//! { "source": "cursor", "kind": "task_finished", "title": "Refactor done" }
//! ```
//!
//! **Claude Code hooks** can POST their raw hook input unchanged (it carries
//! `hook_event_name`, `message`, `cwd`, `session_id`); this module recognises
//! that shape and turns it into a well-titled notification.

use notchforge_core::apps::{AppRef, KnownApp};
use notchforge_core::model::{NewNotification, NotificationKind, Origin, Priority};
use notchforge_core::time::now_ms;
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct AgentPayload {
    /// `claude`, `chatgpt`, `cursor`, `zed`, `opencode`, `qoder`, `agent`, or any name.
    #[serde(alias = "agent", alias = "app")]
    pub source: Option<String>,
    /// `message`, `task_finished`, `needs_input`, `error`, `tool_call`, `info`.
    #[serde(alias = "type", alias = "event")]
    pub kind: Option<String>,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    /// `last-assistant-message` is what the Codex CLI `notify` hook sends.
    #[serde(alias = "message", alias = "text", alias = "last-assistant-message")]
    pub body: Option<String>,
    pub priority: Option<String>,
    #[serde(alias = "session_id", alias = "conversation_id", alias = "session", alias = "turn-id")]
    pub thread: Option<String>,
    pub url: Option<String>,
    /// Unix seconds or milliseconds.
    pub timestamp: Option<i64>,

    // ── Claude Code hook input ──
    pub hook_event_name: Option<String>,
    pub cwd: Option<String>,
    pub tool_name: Option<String>,
}

/// Single event or batch.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(Box<AgentPayload>),
    Many(Vec<AgentPayload>),
}

impl OneOrMany {
    pub fn into_vec(self) -> Vec<AgentPayload> {
        match self {
            OneOrMany::One(p) => vec![*p],
            OneOrMany::Many(v) => v,
        }
    }
}

/// Overrides supplied out-of-band (HTTP query string `?source=claude&kind=stop`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overrides {
    pub source: Option<String>,
    pub kind: Option<String>,
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn project_name(cwd: &str) -> Option<String> {
    std::path::Path::new(cwd).file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Map a Claude Code hook event name to a kind.
fn hook_kind(event: &str) -> NotificationKind {
    match event {
        "Notification" => NotificationKind::NeedsInput,
        "Stop" | "SubagentStop" | "SessionEnd" => NotificationKind::TaskFinished,
        "PreToolUse" | "PostToolUse" => NotificationKind::ToolCall,
        _ => NotificationKind::Info,
    }
}

impl AgentPayload {
    pub fn into_notification(self, origin: Origin, overrides: &Overrides) -> NewNotification {
        let source =
            overrides.source.clone().or(non_empty(self.source)).unwrap_or_else(|| {
                if self.hook_event_name.is_some() { "claude".to_owned() } else { "agent".to_owned() }
            });
        let app = match KnownApp::from_key(&source) {
            Some(k) => AppRef::known(k),
            None => AppRef { known: Some(KnownApp::GenericAgent), bundle_id: None, display_name: source.clone() },
        };

        let hook = non_empty(self.hook_event_name);
        let kind = overrides
            .kind
            .as_deref()
            .or(self.kind.as_deref())
            .map(NotificationKind::parse)
            .or_else(|| hook.as_deref().map(hook_kind))
            .unwrap_or(NotificationKind::Info);

        let project = self.cwd.as_deref().and_then(project_name);
        let title = non_empty(self.title).unwrap_or_else(|| {
            let what = match kind {
                NotificationKind::TaskFinished => "Finished",
                NotificationKind::NeedsInput => "Needs your input",
                NotificationKind::Error => "Error",
                NotificationKind::ToolCall => "Running a tool",
                NotificationKind::Message => "New message",
                NotificationKind::Info => "Update",
            };
            match &project {
                Some(p) => format!("{what} · {p}"),
                None => what.to_owned(),
            }
        });
        let body = non_empty(self.body)
            .or_else(|| self.tool_name.clone().map(|t| format!("Tool: {t}")))
            .unwrap_or_else(|| match kind {
                NotificationKind::TaskFinished => format!("{} finished responding.", app.display_name),
                _ => String::new(),
            });
        let subtitle = non_empty(self.subtitle).or(if hook.is_some() { project.clone() } else { None });
        let timestamp_ms = match self.timestamp {
            Some(t) if t > 10_000_000_000 => t,
            Some(t) if t > 0 => t * 1000,
            _ => now_ms(),
        };
        NewNotification {
            app,
            origin,
            kind,
            priority: self.priority.as_deref().and_then(Priority::parse),
            title,
            subtitle,
            body,
            timestamp_ms,
            thread: non_empty(self.thread),
            url: non_empty(self.url),
        }
    }
}

/// Parse a JSON document (object or array) into notifications.
pub fn parse_events(json: &[u8], origin: Origin, overrides: &Overrides) -> Result<Vec<NewNotification>, String> {
    let parsed: OneOrMany = serde_json::from_slice(json).map_err(|e| format!("invalid JSON: {e}"))?;
    let events: Vec<_> = parsed.into_vec().into_iter().map(|p| p.into_notification(origin, overrides)).collect();
    if events.is_empty() {
        return Err("no events".to_owned());
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_event() {
        let ev = parse_events(
            br#"{"source":"cursor","kind":"task_finished","title":"Done"}"#,
            Origin::AgentWebhook,
            &Overrides::default(),
        )
        .expect("parse");
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].app.known, Some(KnownApp::Cursor));
        assert_eq!(ev[0].kind, NotificationKind::TaskFinished);
        assert_eq!(ev[0].title, "Done");
    }

    #[test]
    fn claude_code_hook_input() {
        let json = br#"{
            "session_id": "abc123",
            "transcript_path": "/tmp/t.jsonl",
            "cwd": "/Users/me/code/notchforge",
            "hook_event_name": "Notification",
            "message": "Claude needs your permission to use Bash"
        }"#;
        let ev = parse_events(json, Origin::AgentWebhook, &Overrides::default()).expect("parse");
        let n = &ev[0];
        assert_eq!(n.app.known, Some(KnownApp::Claude));
        assert_eq!(n.kind, NotificationKind::NeedsInput);
        assert_eq!(n.title, "Needs your input · notchforge");
        assert_eq!(n.body, "Claude needs your permission to use Bash");
        assert_eq!(n.thread.as_deref(), Some("abc123"));
        assert_eq!(n.subtitle.as_deref(), Some("notchforge"));

        let stop = br#"{"hook_event_name":"Stop","cwd":"/x/proj","session_id":"s"}"#;
        let n = &parse_events(stop, Origin::AgentWebhook, &Overrides::default()).expect("parse")[0];
        assert_eq!(n.kind, NotificationKind::TaskFinished);
        assert_eq!(n.body, "Claude finished responding.");
    }

    #[test]
    fn overrides_and_aliases() {
        let json =
            br#"[{"agent":"whatever","type":"error","message":"boom","priority":"critical","timestamp":1700000000}]"#;
        let o = Overrides { source: Some("qoder".into()), kind: None };
        let n = &parse_events(json, Origin::AgentSocket, &o).expect("parse")[0];
        assert_eq!(n.app.known, Some(KnownApp::Qoder));
        assert_eq!(n.kind, NotificationKind::Error);
        assert_eq!(n.priority, Some(Priority::Critical));
        assert_eq!(n.timestamp_ms, 1_700_000_000_000);
        assert_eq!(n.title, "Error");
    }

    #[test]
    fn codex_notify_payload() {
        let json = br#"{"type":"agent-turn-complete","turn-id":"t1","input-messages":["fix it"],"last-assistant-message":"Fixed the bug."}"#;
        let o = Overrides { source: Some("chatgpt".into()), kind: None };
        let n = &parse_events(json, Origin::AgentWebhook, &o).expect("parse")[0];
        assert_eq!(n.app.known, Some(KnownApp::ChatGpt));
        assert_eq!(n.kind, NotificationKind::TaskFinished);
        assert_eq!(n.body, "Fixed the bug.");
        assert_eq!(n.title, "Finished");
    }

    #[test]
    fn unknown_source_is_generic_agent_with_its_name() {
        let n = &parse_events(br#"{"source":"aider","title":"t"}"#, Origin::AgentInbox, &Overrides::default())
            .expect("parse")[0];
        assert_eq!(n.app.known, Some(KnownApp::GenericAgent));
        assert_eq!(n.app.display_name, "aider");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_events(b"not json", Origin::AgentWebhook, &Overrides::default()).is_err());
        assert!(parse_events(b"[]", Origin::AgentWebhook, &Overrides::default()).is_err());
    }
}
