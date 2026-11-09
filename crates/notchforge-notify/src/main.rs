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
    --input <N>         Tokens used, reported to the Agents tab (counts instead of showing a notification)
    --output <N>        Output tokens
    --cache-read <N>    Cache-read tokens
    --cache-write <N>   Cache-write tokens
    --model <NAME>      The model the tokens were spent on
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
    /// Start a timer from a natural duration ("25m", "1h30m", "90 s").
    timer: Option<String>,
    /// Reported token usage (see the usage ingest).
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    model: Option<String>,
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
            "--input" => a.input = Some(value("--input")?.parse().map_err(|_| "invalid --input")?),
            "--output" => a.output = Some(value("--output")?.parse().map_err(|_| "invalid --output")?),
            "--cache-read" => a.cache_read = Some(value("--cache-read")?.parse().map_err(|_| "invalid --cache-read")?),
            "--cache-write" => {
                a.cache_write = Some(value("--cache-write")?.parse().map_err(|_| "invalid --cache-write")?)
            }
            "--model" => a.model = Some(value("--model")?),
    "--timer" => a.timer = Some(value("--timer")?),
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
    if a.input.is_some() || a.output.is_some() || a.cache_read.is_some() || a.cache_write.is_some() {
        let mut t = serde_json::Map::new();
        if let Some(v) = a.input {
            t.insert("input".into(), v.into());
        }
        if let Some(v) = a.output {
            t.insert("output".into(), v.into());
        }
        if let Some(v) = a.cache_read {
            t.insert("cache_read".into(), v.into());
        }
        if let Some(v) = a.cache_write {
            t.insert("cache_write".into(), v.into());
        }
        obj.insert("tokens".into(), serde_json::Value::Object(t));
    }
    if let Some(m) = &a.model {
        obj.insert("model".into(), m.clone().into());
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
    // A timer request rides the same delivery: the engine recognizes the
    // thread and starts the countdown instead of showing a notification.
    if let Some(spec) = args.timer.clone() {
        let (ms, label) = match parse_duration(&spec) {
            Some(v) => v,
            None => return fail(format!("could not read the duration: {spec}"), args.best_effort),
        };
        let body = serde_json::json!({
            "source": "notchforge",
            "kind": "Info",
            "thread": "notchforge-timer",
            "title": label,
            "body": ms.to_string(),
        })
        .to_string();
        return deliver(&args, &body);
    }
    let body = match build_body(&args, stdin.as_deref()) {
        Ok(b) => b.to_string(),
        Err(e) => return fail(e, args.best_effort),
    };
    deliver(&args, &body)
}

/// Send the body through the transports (HTTP, socket, inbox).
fn deliver(args: &Args, body: &str) -> ExitCode {
    let fail = |msg: String, best_effort: bool| {
        eprintln!("notchforge-notify: {msg}");
        if best_effort { ExitCode::SUCCESS } else { ExitCode::FAILURE }
    };
    let port =
        args.port.or_else(|| std::env::var("NOTCHFORGE_PORT").ok().and_then(|p| p.parse().ok())).unwrap_or(47815);
    let token = args.token.clone().or_else(|| std::env::var("NOTCHFORGE_TOKEN").ok());
    let mut query = Vec::new();
    if let Some(s) = &args.source {
        query.push(format!("source={}", percent_encode(s)));
    }
    let query = if query.is_empty() { String::new() } else { format!("?{}", query.join("&")) };
    let http = send_http(port, token.as_deref(), &query, body);
    if http.is_ok() {
        return ExitCode::SUCCESS;
    }
    if send_socket(body).is_ok() {
        return ExitCode::SUCCESS;
    }
    match append_inbox(body) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(format!("could not deliver ({}; inbox: {e})", http.err().unwrap_or_default()), args.best_effort),
    }
}

/// "25m" / "1h30m" / "90s" / "25 min" → (ms, label).
fn parse_duration(spec: &str) -> Option<(i64, String)> {
    let mut total = 0i64;
    let mut parts = Vec::new();
    let mut num = String::new();
    let mut unit = String::new();
    let mut found = false;
    for c in spec.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() {
            if !unit.is_empty() {
                let (ms, name) = push_part(&num, &unit)?;
                total += ms;
                parts.push(format!("{num}{name}"));
                num.clear();
                unit.clear();
            }
            num.push(c);
        } else if c.is_alphabetic() {
            unit.push(c);
        } else if !num.is_empty() && !unit.is_empty() {
            let (ms, name) = push_part(&num, &unit)?;
            total += ms;
            parts.push(format!("{num}{name}"));
            num.clear();
            unit.clear();
            found = true;
        }
    }
    if !found && total == 0 {
        return None;
    }
    Some((total, parts.join(" ")))
}

fn push_part(num: &str, unit: &str) -> Option<(i64, &'static str)> {
    let n: i64 = num.parse().ok()?;
    match unit.trim_start().to_lowercase().as_str() {
        u if u.starts_with('h') => Some((n * 3_600_000, "h")),
        u if u.starts_with('m') => Some((n * 60_000, "m")),
        u if u.starts_with('s') => Some((n * 1_000, "s")),
        _ => None,
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
    fn timer_specs_parse() {
        assert_eq!(parse_duration("25m"), Some((25 * 60_000, "25m".into())));
        assert_eq!(parse_duration("1h30m"), Some((90 * 60_000, "1h 30m".into())));
        assert_eq!(parse_duration("90s"), Some((90_000, "90s".into())));
        assert_eq!(parse_duration("25 min"), Some((25 * 60_000, "25m".into())));
        assert_eq!(parse_duration("banana"), None);
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn encoding() {
        assert_eq!(percent_encode("a b/c"), "a%20b%2Fc");
    }
}
