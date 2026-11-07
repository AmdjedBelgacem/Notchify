//! The MCP JSON-RPC 2.0 layer: one request line in, at most one response out.

use crate::http::{SendError, Sender};
use serde_json::{Map, Value, json};

pub const LATEST_PROTOCOL: &str = "2025-06-18";
pub const SUPPORTED_PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// The `kind` values the app's payload understands (see
/// `notchforge-providers/src/agents/payload.rs`).
pub const KINDS: &[&str] = &["message", "task_finished", "needs_input", "error", "tool_call", "info"];

/// Map an MCP client name (`clientInfo.name`) to a payload `source`.
pub fn source_for_client(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if n.contains("claude") {
        "claude"
    } else if n.contains("codex") {
        "chatgpt"
    } else if n.contains("cursor") {
        "cursor"
    } else if n.contains("gemini") {
        "gemini"
    } else {
        "mcp"
    }
}

pub struct Server<S: Sender> {
    sender: S,
    source: &'static str,
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

fn text_result(text: impl Into<String>, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }], "isError": is_error })
}

fn tools() -> Value {
    json!([
        {
            "name": "notch_notify",
            "title": "Notify in the notch",
            "description": "Show a notification in the Notchisland notch on the user's Mac. Use it to tell the user something worth looking at now (a question, a blocker, an error, a result).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short headline." },
                    "body": { "type": "string", "description": "Optional detail text." },
                    "kind": { "type": "string", "enum": KINDS, "description": "Event kind; defaults to info. needs_input and error are shown with high priority." },
                    "url": { "type": "string", "description": "Optional link or file path opened by the notification's Open action." }
                },
                "required": ["title"],
                "additionalProperties": false
            }
        },
        {
            "name": "notch_progress",
            "title": "Report progress in the notch",
            "description": "Report progress on a long-running task (e.g. 3 of 10 steps) in the Notchisland notch. Updates for the same task are grouped together.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "What is being worked on." },
                    "done": { "type": "number", "minimum": 0, "description": "Units completed." },
                    "total": { "type": "number", "exclusiveMinimum": 0, "description": "Total units." },
                    "note": { "type": "string", "description": "Optional note about the current step." }
                },
                "required": ["task", "done", "total"],
                "additionalProperties": false
            }
        },
        {
            "name": "notch_finished",
            "title": "Announce a finished task in the notch",
            "description": "Tell the user, via the Notchisland notch, that a task is finished, with the files changed and the test outcome.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "summary": { "type": "string", "description": "One-line summary of what was done." },
                    "files_changed": { "type": "array", "items": { "type": "string" }, "description": "Paths of files that changed." },
                    "tests": { "type": "string", "enum": ["passed", "failed", "none"], "description": "Test outcome." }
                },
                "required": ["summary"],
                "additionalProperties": false
            }
        }
    ])
}

fn req_str(args: &Map<String, Value>, key: &str) -> Result<String, String> {
    match args.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.trim().to_owned()),
        Some(Value::String(_)) => Err(format!("`{key}` must not be empty")),
        Some(_) => Err(format!("`{key}` must be a string")),
        None => Err(format!("missing required argument `{key}`")),
    }
}

fn opt_str(args: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.trim().to_owned()).filter(|s| !s.is_empty())),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

fn req_num(args: &Map<String, Value>, key: &str) -> Result<f64, String> {
    match args.get(key) {
        Some(Value::Number(n)) => {
            n.as_f64().filter(|f| f.is_finite()).ok_or_else(|| format!("`{key}` is out of range"))
        }
        Some(_) => Err(format!("`{key}` must be a number")),
        None => Err(format!("missing required argument `{key}`")),
    }
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

impl<S: Sender> Server<S> {
    pub fn new(sender: S) -> Self {
        Self { sender, source: "mcp" }
    }

    /// The payload `source` events are posted with (set from `initialize`).
    #[cfg(test)]
    pub fn source(&self) -> &'static str {
        self.source
    }

    /// Handle one line of input. Returns the response to write, if any
    /// (notifications get none).
    pub fn handle_line(&mut self, line: &str) -> Option<Value> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(v) => self.handle(v),
            Err(e) => Some(err(Value::Null, PARSE_ERROR, format!("parse error: {e}"))),
        }
    }

    pub fn handle(&mut self, msg: Value) -> Option<Value> {
        let Value::Object(obj) = msg else {
            return Some(err(
                Value::Null,
                INVALID_REQUEST,
                "expected a JSON-RPC request object (batches are not supported)",
            ));
        };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // A response from the client (we never send requests) or garbage.
            return id.filter(|i| !i.is_null()).map(|i| err(i, INVALID_REQUEST, "missing method"));
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else {
            // Notification: `notifications/initialized`, `notifications/cancelled`, … — no reply.
            return None;
        };
        Some(match method {
            "initialize" => self.initialize(id, &params),
            "ping" => ok(id, json!({})),
            "tools/list" => ok(id, json!({ "tools": tools() })),
            "tools/call" => self.call(id, &params),
            other => err(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
        })
    }

    fn initialize(&mut self, id: Value, params: &Value) -> Value {
        if let Some(name) = params.pointer("/clientInfo/name").and_then(Value::as_str) {
            self.source = source_for_client(name);
        }
        let requested = params.get("protocolVersion").and_then(Value::as_str);
        let version = requested.filter(|v| SUPPORTED_PROTOCOLS.contains(v)).unwrap_or(LATEST_PROTOCOL);
        ok(
            id,
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "notchisland", "title": "Notchisland", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Use these tools to reach the user through the Notchisland notch on their Mac: notch_finished when a task is done, notch_notify when you need their attention, notch_progress for long-running work."
            }),
        )
    }

    fn call(&mut self, id: Value, params: &Value) -> Value {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return err(id, INVALID_PARAMS, "tools/call needs a `name`");
        };
        let empty = Map::new();
        let args = match params.get("arguments") {
            None | Some(Value::Null) => &empty,
            Some(Value::Object(m)) => m,
            Some(_) => return err(id, INVALID_PARAMS, "`arguments` must be an object"),
        };
        let built = match name {
            "notch_notify" => self.build_notify(args),
            "notch_progress" => self.build_progress(args),
            "notch_finished" => self.build_finished(args),
            other => return err(id, INVALID_PARAMS, format!("unknown tool: {other}")),
        };
        let (event, confirmation) = match built {
            Ok(x) => x,
            Err(e) => return err(id, INVALID_PARAMS, format!("{name}: {e}")),
        };
        let result = match self.sender.send(&event) {
            Ok(()) => text_result(confirmation, false),
            Err(SendError::NotRunning) => text_result("Notchisland is not running", true),
            Err(SendError::Failed(e)) => text_result(format!("Could not reach Notchisland: {e}"), true),
        };
        ok(id, result)
    }

    fn base(&self, kind: &str, title: String) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("source".into(), self.source.into());
        m.insert("kind".into(), kind.into());
        m.insert("title".into(), title.into());
        m
    }

    fn build_notify(&self, args: &Map<String, Value>) -> Result<(Value, String), String> {
        let title = req_str(args, "title")?;
        let kind = match opt_str(args, "kind")? {
            Some(k) if KINDS.contains(&k.as_str()) => k,
            Some(k) => return Err(format!("`kind` must be one of {}, got {k:?}", KINDS.join(", "))),
            None => "info".to_owned(),
        };
        let mut m = self.base(&kind, title.clone());
        if let Some(b) = opt_str(args, "body")? {
            m.insert("body".into(), b.into());
        }
        if let Some(u) = opt_str(args, "url")? {
            m.insert("url".into(), u.into());
        }
        Ok((Value::Object(m), format!("Shown in the notch: {title}")))
    }

    /// The payload has no progress kind or progress fields, so progress is an
    /// `info` event titled "task — done/total (pct%)", grouped per task via `thread`.
    fn build_progress(&self, args: &Map<String, Value>) -> Result<(Value, String), String> {
        let task = req_str(args, "task")?;
        let done = req_num(args, "done")?;
        let total = req_num(args, "total")?;
        if done < 0.0 {
            return Err("`done` must be >= 0".into());
        }
        if total <= 0.0 {
            return Err("`total` must be > 0".into());
        }
        let note = opt_str(args, "note")?;
        let pct = ((done / total) * 100.0).clamp(0.0, 100.0).round();
        let title = format!("{task} — {}/{} ({}%)", fmt_num(done), fmt_num(total), pct as i64);
        let mut m = self.base("info", title.clone());
        if let Some(n) = note {
            m.insert("body".into(), n.into());
        }
        m.insert("thread".into(), format!("mcp-progress:{task}").into());
        Ok((Value::Object(m), format!("Progress shown in the notch: {title}")))
    }

    fn build_finished(&self, args: &Map<String, Value>) -> Result<(Value, String), String> {
        let summary = req_str(args, "summary")?;
        let files: Vec<String> = match args.get("files_changed") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| v.as_str().map(|s| s.trim().to_owned()).ok_or("`files_changed` must be an array of strings"))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect(),
            Some(_) => return Err("`files_changed` must be an array of strings".into()),
        };
        let tests = match opt_str(args, "tests")? {
            Some(t) if ["passed", "failed", "none"].contains(&t.as_str()) => Some(t),
            Some(t) => return Err(format!("`tests` must be passed, failed or none, got {t:?}")),
            None => None,
        };
        let mut lines = Vec::new();
        if !files.is_empty() {
            const SHOWN: usize = 8;
            let mut list = files.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
            if files.len() > SHOWN {
                list.push_str(&format!(", +{} more", files.len() - SHOWN));
            }
            let noun = if files.len() == 1 { "file" } else { "files" };
            lines.push(format!("{} {noun} changed: {list}", files.len()));
        }
        match tests.as_deref() {
            Some("passed") => lines.push("Tests: passed".into()),
            Some("failed") => lines.push("Tests: FAILED".into()),
            Some(_) => lines.push("Tests: none run".into()),
            None => {}
        }
        // Failed tests are worth a louder notification.
        let mut m = self.base("task_finished", summary.clone());
        if tests.as_deref() == Some("failed") {
            m.insert("priority".into(), "high".into());
        }
        if !lines.is_empty() {
            m.insert("body".into(), lines.join("\n").into());
        }
        Ok((Value::Object(m), format!("Finished shown in the notch: {summary}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    type Sent = Rc<RefCell<Vec<Value>>>;

    fn server(result: Result<(), SendError>) -> (Server<impl Sender>, Sent) {
        let sent: Sent = Rc::default();
        let log = sent.clone();
        let s = Server::new(move |ev: &Value| {
            log.borrow_mut().push(ev.clone());
            result.clone()
        });
        (s, sent)
    }

    fn rpc(s: &mut Server<impl Sender>, v: Value) -> Value {
        s.handle_line(&v.to_string()).expect("a response")
    }

    fn init(s: &mut Server<impl Sender>, client: &str, version: &str) -> Value {
        rpc(
            s,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion": version, "capabilities": {}, "clientInfo": {"name": client, "version": "1"}}}),
        )
    }

    fn call(s: &mut Server<impl Sender>, name: &str, args: Value) -> Value {
        rpc(s, json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":name,"arguments":args}}))
    }

    #[test]
    fn initialize_negotiates_version_and_maps_source() {
        let (mut s, _) = server(Ok(()));
        let r = init(&mut s, "claude-code", "2025-06-18");
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["serverInfo"]["name"], "notchisland");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        assert_eq!(s.source(), "claude");

        let (mut s, _) = server(Ok(()));
        assert_eq!(init(&mut s, "codex-mcp-client", "2024-11-05")["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(s.source(), "chatgpt");

        let (mut s, _) = server(Ok(()));
        assert_eq!(init(&mut s, "zed", "1999-01-01")["result"]["protocolVersion"], LATEST_PROTOCOL);
        assert_eq!(s.source(), "mcp");
    }

    #[test]
    fn client_name_mapping() {
        assert_eq!(source_for_client("Claude Code"), "claude");
        assert_eq!(source_for_client("codex"), "chatgpt");
        assert_eq!(source_for_client("cursor-vscode"), "cursor");
        assert_eq!(source_for_client("gemini-cli-mcp-client"), "gemini");
        assert_eq!(source_for_client("windsurf"), "mcp");
    }

    #[test]
    fn notifications_and_ping() {
        let (mut s, _) = server(Ok(()));
        assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        assert!(s.handle_line("   ").is_none());
        let r = rpc(&mut s, json!({"jsonrpc":"2.0","id":"p","method":"ping"}));
        assert_eq!(r["id"], "p");
        assert_eq!(r["result"], json!({}));
    }

    #[test]
    fn protocol_errors() {
        let (mut s, _) = server(Ok(()));
        assert_eq!(s.handle_line("{nope").unwrap()["error"]["code"], PARSE_ERROR);
        assert_eq!(s.handle_line("[]").unwrap()["error"]["code"], INVALID_REQUEST);
        let r = rpc(&mut s, json!({"jsonrpc":"2.0","id":3,"method":"resources/list"}));
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(r["id"], 3);
    }

    #[test]
    fn tools_list_shape() {
        let (mut s, _) = server(Ok(()));
        let r = rpc(&mut s, json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
        let tools = r["result"]["tools"].as_array().expect("tools array");
        let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["notch_notify", "notch_progress", "notch_finished"]);
        for t in tools {
            assert!(t["description"].is_string());
            assert_eq!(t["inputSchema"]["type"], "object");
            assert!(t["inputSchema"]["required"].is_array());
        }
        assert_eq!(tools[0]["inputSchema"]["properties"]["kind"]["enum"], json!(KINDS));
        assert_eq!(tools[1]["inputSchema"]["required"], json!(["task", "done", "total"]));
        assert_eq!(tools[2]["inputSchema"]["properties"]["tests"]["enum"], json!(["passed", "failed", "none"]));
    }

    #[test]
    fn notify_posts_event_with_client_source() {
        let (mut s, sent) = server(Ok(()));
        init(&mut s, "cursor", "2025-06-18");
        let r = call(
            &mut s,
            "notch_notify",
            json!({"title":"Need a decision","body":"A or B?","kind":"needs_input","url":"https://x"}),
        );
        assert_eq!(r["result"]["isError"], false);
        assert_eq!(r["result"]["content"][0]["type"], "text");
        let ev = &sent.borrow()[0];
        assert_eq!(
            ev,
            &json!({"source":"cursor","kind":"needs_input","title":"Need a decision","body":"A or B?","url":"https://x"})
        );
    }

    #[test]
    fn argument_validation() {
        let (mut s, sent) = server(Ok(()));
        for (tool, args) in [
            ("notch_notify", json!({})),
            ("notch_notify", json!({"title": 5})),
            ("notch_notify", json!({"title": "  "})),
            ("notch_notify", json!({"title": "t", "kind": "party"})),
            ("notch_progress", json!({"task": "t", "done": 1})),
            ("notch_progress", json!({"task": "t", "done": "1", "total": 2})),
            ("notch_progress", json!({"task": "t", "done": 1, "total": 0})),
            ("notch_finished", json!({"summary": "s", "files_changed": "a.rs"})),
            ("notch_finished", json!({"summary": "s", "files_changed": [1]})),
            ("notch_finished", json!({"summary": "s", "tests": "maybe"})),
            ("nope", json!({})),
        ] {
            let r = call(&mut s, tool, args.clone());
            assert_eq!(r["error"]["code"], INVALID_PARAMS, "{tool} {args}");
        }
        let r = rpc(
            &mut s,
            json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"notch_notify","arguments":[1]}}),
        );
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
        let r = rpc(&mut s, json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{}}));
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
        assert!(sent.borrow().is_empty(), "nothing is posted on invalid input");
    }

    #[test]
    fn progress_event() {
        let (mut s, sent) = server(Ok(()));
        let r =
            call(&mut s, "notch_progress", json!({"task":"Migrating tests","done":3,"total":10,"note":"auth module"}));
        assert_eq!(r["result"]["isError"], false);
        let ev = &sent.borrow()[0];
        assert_eq!(ev["kind"], "info");
        assert_eq!(ev["title"], "Migrating tests — 3/10 (30%)");
        assert_eq!(ev["body"], "auth module");
        assert_eq!(ev["thread"], "mcp-progress:Migrating tests");
        assert_eq!(ev["source"], "mcp");
    }

    #[test]
    fn finished_event_body() {
        let (mut s, sent) = server(Ok(()));
        init(&mut s, "gemini-cli", "2025-06-18");
        call(
            &mut s,
            "notch_finished",
            json!({"summary":"Refactor done","files_changed":["a.rs","b.rs"],"tests":"failed"}),
        );
        call(&mut s, "notch_finished", json!({"summary":"Docs"}));
        let sent = sent.borrow();
        assert_eq!(sent[0]["kind"], "task_finished");
        assert_eq!(sent[0]["source"], "gemini");
        assert_eq!(sent[0]["title"], "Refactor done");
        assert_eq!(sent[0]["body"], "2 files changed: a.rs, b.rs\nTests: FAILED");
        assert_eq!(sent[0]["priority"], "high");
        assert!(sent[1].get("body").is_none());
        assert!(sent[1].get("priority").is_none());
    }

    #[test]
    fn app_not_running_is_tool_error() {
        let (mut s, _) = server(Err(SendError::NotRunning));
        let r = call(&mut s, "notch_notify", json!({"title":"hi"}));
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(r["result"]["content"][0]["text"], "Notchisland is not running");

        let (mut s, _) = server(Err(SendError::Failed("401".into())));
        let r = call(&mut s, "notch_finished", json!({"summary":"x"}));
        assert_eq!(r["result"]["isError"], true);
    }
}
