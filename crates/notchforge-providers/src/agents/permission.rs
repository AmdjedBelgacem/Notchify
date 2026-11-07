//! Tool-permission requests held for the notch.
//!
//! Claude Code's `PermissionRequest` hook posts to the webhook and waits for
//! the reply. The webhook parks the request here, the engine shows an
//! Allow / Deny prompt, and the person's answer ([`decide`]) wakes the waiting
//! connection, which returns it to the agent. A request nobody answers in
//! time falls back to the agent's own prompt in its terminal.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::oneshot;

static PENDING: Mutex<Option<HashMap<String, oneshot::Sender<bool>>>> = Mutex::new(None);
static NEXT: AtomicU64 = AtomicU64::new(1);

/// Park a request: its id and the receiver of the answer.
pub fn register() -> (String, oneshot::Receiver<bool>) {
    let id = format!("p{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let (tx, rx) = oneshot::channel();
    let mut map = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    map.get_or_insert_with(HashMap::new).insert(id.clone(), tx);
    (id, rx)
}

/// Answer a parked request. False when it already ended (timed out).
pub fn decide(id: &str, allow: bool) -> bool {
    let tx = PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|m| m.remove(id));
    match tx {
        Some(tx) => tx.send(allow).is_ok(),
        None => false,
    }
}

/// Forget a request that ended without an answer.
pub fn forget(id: &str) {
    if let Some(m) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(id);
    }
}

/// The hook's reply for Claude Code: allow, or deny with a reason.
pub fn claude_reply(allow: bool) -> serde_json::Value {
    let decision = if allow {
        serde_json::json!({ "behavior": "allow" })
    } else {
        serde_json::json!({ "behavior": "deny", "message": "Denied from the notch." })
    };
    serde_json::json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision } })
}

/// In a few words, what a tool call asks to do (the prompt's body).
pub fn describe(tool: &str, input: &serde_json::Value) -> String {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_owned();
    let text = match tool {
        "Bash" => s("command"),
        "Edit" | "MultiEdit" | "Write" | "Read" | "NotebookEdit" => {
            let p = s("file_path");
            if p.is_empty() { s("notebook_path") } else { p }
        }
        "WebFetch" => s("url"),
        "WebSearch" => s("query"),
        _ => {
            let d = s("description");
            if d.is_empty() { input.to_string() } else { d }
        }
    };
    let one_line = text.lines().next().unwrap_or("").to_owned();
    if one_line.chars().count() > 160 {
        format!("{}…", one_line.chars().take(159).collect::<String>())
    } else {
        one_line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_answer_reaches_the_waiting_request_once() {
        let (id, rx) = register();
        assert!(decide(&id, true));
        assert!(rx.await.expect("answer"));
        assert!(!decide(&id, false), "already answered");
        let (id2, _rx2) = register();
        forget(&id2);
        assert!(!decide(&id2, true));
    }

    #[test]
    fn replies_and_descriptions() {
        assert_eq!(claude_reply(true)["hookSpecificOutput"]["decision"]["behavior"], "allow");
        assert_eq!(claude_reply(false)["hookSpecificOutput"]["decision"]["behavior"], "deny");
        let cmd = serde_json::json!({ "command": "cargo test\necho hi" });
        assert_eq!(describe("Bash", &cmd), "cargo test");
        assert_eq!(describe("Edit", &serde_json::json!({ "file_path": "/a/b.rs" })), "/a/b.rs");
    }
}
