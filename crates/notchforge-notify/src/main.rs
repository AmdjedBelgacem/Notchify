//! `notchforge-notify` — push an event to NotchForge from any script or hook.
//!
//! ```sh
//! notchforge-notify --source cursor --kind task_finished --title "Refactor done"
//! # Claude Code hook (raw hook JSON on stdin):
//! notchforge-notify --source claude --stdin
//! ```
//!
//! Delivery order: HTTP webhook → Unix socket → append to the inbox file, so
//! events are not lost even if NotchForge is restarting. Uses only `std`
//! networking to stay tiny and start fast (hooks run on every agent turn).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const USAGE: &str = "\
notchforge-notify — send a notification to NotchForge

USAGE:
    notchforge-notify [OPTIONS]

OPTIONS:
    --source <NAME>     claude | chatgpt | cursor | zed | opencode | qoder | agent | any name
    --kind <KIND>       message | task_finished | needs_input | error | tool_call | info
    --title <TEXT>      Title
    --body <TEXT>       Body / message
    --priority <P>      low | normal | high | critical
    --thread <ID>       Conversation / session id (used for grouping)
    --url <URL>         Link or file path opened by \"Open\"
    --stdin             Read a JSON event (or a Claude Code hook payload) from stdin
    --port <N>          Webhook port (default 47815 or $NOTCHFORGE_PORT)
    --token <T>         Bearer token (or $NOTCHFORGE_TOKEN)
    --best-effort       Always exit 0 (recommended in hooks)
    '{...}'             A JSON event as the last argument (Codex CLI `notify` style)
    -h, --help          Show this help";

#[derive(Default)]
struct Args {
    fields: Vec<(&'static str, String)>,
    stdin: bool,
    port: Option<u16>,
    token: Option<String>,
    best_effort: bool,
    source: Option<String>,
    kind: Option<String>,
    json_arg: Option<String>,
}

fn parse() -> Result<Option<Args>, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--source" => a.source = Some(value("--source")?),
            "--kind" => a.kind = Some(value("--kind")?),
            "--title" => a.fields.push(("title", value("--title")?)),
            "--body" | "--message" => a.fields.push(("body", value("--body")?)),
            "--priority" => a.fields.push(("priority", value("--priority")?)),
            "--thread" => a.fields.push(("thread", value("--thread")?)),
            "--url" => a.fields.push(("url", value("--url")?)),
            "--port" => a.port = Some(value("--port")?.parse().map_err(|_| "invalid --port")?),
            "--token" => a.token = Some(value("--token")?),
            "--stdin" => a.stdin = true,
            "--best-effort" => a.best_effort = true,
            "-h" | "--help" => return Ok(None),
            // A bare JSON argument (the Codex CLI `notify` hook passes one).
            json if json.trim_start().starts_with('{') => a.json_arg = Some(json.to_owned()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Some(a))
}

fn data_dir() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("NOTCHFORGE_HOME") {
        return Some(PathBuf::from(h));
    }
    dirs::data_dir().map(|d| d.join("NotchForge"))
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Build the JSON body. With `--stdin`, CLI fields override/augment the input.
fn build_body(a: &Args, stdin_json: Option<&str>) -> Result<serde_json::Value, String> {
    let mut obj = match stdin_json.map(str::trim).filter(|s| !s.is_empty()) {
        Some(text) => match serde_json::from_str::<serde_json::Value>(text) {
            Ok(serde_json::Value::Object(m)) => m,
            Ok(_) => return Err("stdin must contain a JSON object".into()),
            Err(e) => return Err(format!("invalid JSON on stdin: {e}")),
        },
        None => serde_json::Map::new(),
    };
    for (k, v) in &a.fields {
        obj.insert((*k).to_owned(), serde_json::Value::String(v.clone()));
    }
    if let Some(s) = &a.source {
        obj.insert("source".into(), s.clone().into());
    }
    if let Some(k) = &a.kind {
        obj.insert("kind".into(), k.clone().into());
    }
    if obj.is_empty() {
        return Err("nothing to send (use --title/--body or --stdin)".into());
    }
    Ok(serde_json::Value::Object(obj))
}

fn send_http(port: u16, token: Option<&str>, query: &str, body: &str) -> Result<(), String> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok();
    let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let req = format!(
        "POST /v1/notify{query} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut resp = String::new();
    s.read_to_string(&mut resp).map_err(|e| e.to_string())?;
    let status = resp.split_whitespace().nth(1).unwrap_or("");
    if status.starts_with('2') {
        Ok(())
    } else {
        Err(format!("server replied: {}", resp.lines().last().unwrap_or(status)))
    }
}

#[cfg(unix)]
fn send_socket(line: &str) -> Result<(), String> {
    let path = data_dir().ok_or("no data dir")?.join("agent.sock");
    let mut s = std::os::unix::net::UnixStream::connect(path).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok();
    s.write_all(format!("{line}\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 256];
    let n = s.read(&mut buf).map_err(|e| e.to_string())?;
    let reply = String::from_utf8_lossy(&buf[..n]);
    if reply.starts_with("ok") { Ok(()) } else { Err(reply.trim().to_owned()) }
}

#[cfg(not(unix))]
fn send_socket(_line: &str) -> Result<(), String> {
    Err("unsupported".into())
}

fn append_inbox(line: &str) -> Result<(), String> {
    let dir = data_dir().ok_or("no data dir")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("inbox.jsonl"))
        .map_err(|e| e.to_string())?;
    writeln!(f, "{line}").map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(Some(a)) => a,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let fail = |msg: String, best_effort: bool| {
        eprintln!("notchforge-notify: {msg}");
        if best_effort { ExitCode::SUCCESS } else { ExitCode::FAILURE }
    };
    let stdin = if let Some(j) = args.json_arg.clone() {
        Some(j)
    } else if args.stdin {
        let mut s = String::new();
        if let Err(e) = std::io::stdin().read_to_string(&mut s) {
            return fail(format!("reading stdin: {e}"), args.best_effort);
        }
        Some(s)
    } else {
        None
    };
    let body = match build_body(&args, stdin.as_deref()) {
        Ok(b) => b.to_string(),
        Err(e) => return fail(e, args.best_effort),
    };
    let port =
        args.port.or_else(|| std::env::var("NOTCHFORGE_PORT").ok().and_then(|p| p.parse().ok())).unwrap_or(47815);
    let token = args.token.clone().or_else(|| std::env::var("NOTCHFORGE_TOKEN").ok());
    let mut query = Vec::new();
    if let Some(s) = &args.source {
        query.push(format!("source={}", percent_encode(s)));
    }
    let query = if query.is_empty() { String::new() } else { format!("?{}", query.join("&")) };

    let http = send_http(port, token.as_deref(), &query, &body);
    if http.is_ok() {
        return ExitCode::SUCCESS;
    }
    if send_socket(&body).is_ok() {
        return ExitCode::SUCCESS;
    }
    match append_inbox(&body) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(format!("could not deliver ({}; inbox: {e})", http.err().unwrap_or_default()), args.best_effort),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_merges_stdin_and_flags() {
        let a = Args { fields: vec![("title", "T".into())], source: Some("claude".into()), ..Args::default() };
        let v = build_body(&a, Some(r#"{"hook_event_name":"Stop","title":"old"}"#)).expect("body");
        assert_eq!(v["title"], "T");
        assert_eq!(v["source"], "claude");
        assert_eq!(v["hook_event_name"], "Stop");
        assert!(build_body(&Args::default(), None).is_err());
        assert!(build_body(&Args::default(), Some("[1]")).is_err());
    }

    #[test]
    fn encoding() {
        assert_eq!(percent_encode("a b/c"), "a%20b%2Fc");
    }
}
