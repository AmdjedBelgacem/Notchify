//! Loopback HTTP webhook for coding agents.
//!
//! A deliberately tiny HTTP/1.1 server (no framework: we need two routes and
//! want to keep the binary and idle memory small). It binds to `127.0.0.1`
//! only and optionally requires a bearer token.
//!
//! * `POST /v1/notify[?source=claude&kind=stop]` with a JSON body (object or
//!   array, see [`payload`](super::payload)) → `202 {"accepted":N}`
//! * `GET /v1/health` → `200 {"ok":true,"version":"…"}`

use super::payload::{Overrides, parse_events};
use notchforge_core::model::{Origin, ProviderStatus};
use notchforge_core::provider::{
    BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 256 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHead {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub content_length: usize,
    pub authorization: Option<String>,
}

/// Parse the request line and headers (everything before `\r\n\r\n`).
pub fn parse_head(head: &str) -> Result<RequestHead, &'static str> {
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or("empty request")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("missing method")?.to_ascii_uppercase();
    let target = parts.next().ok_or("missing target")?;
    let version = parts.next().ok_or("missing version")?;
    if !version.starts_with("HTTP/1.") {
        return Err("unsupported HTTP version");
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, parse_query(q)),
        None => (target, Vec::new()),
    };
    let mut content_length = 0;
    let mut authorization = None;
    for line in lines.filter(|l| !l.is_empty()) {
        let (name, value) = line.split_once(':').ok_or("malformed header")?;
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => content_length = value.parse().map_err(|_| "bad content-length")?,
            "authorization" => authorization = Some(value.to_owned()),
            "transfer-encoding" if value.eq_ignore_ascii_case("chunked") => {
                return Err("chunked encoding not supported; send Content-Length");
            }
            _ => {}
        }
    }
    Ok(RequestHead { method, path: path.to_owned(), query, content_length, authorization })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(hi), Some(lo)) => {
                    out.push(hi << 4 | lo);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| match kv.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(kv), String::new()),
        })
        .collect()
}

/// Constant-time comparison so the token cannot be guessed byte by byte.
fn token_matches(expected: &str, header: Option<&str>) -> bool {
    let Some(given) = header.and_then(|h| h.strip_prefix("Bearer ")) else { return false };
    let (a, b) = (expected.as_bytes(), given.trim().as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

struct Response {
    status: u16,
    body: String,
}

impl Response {
    fn json(status: u16, body: impl Into<String>) -> Self {
        Self { status, body: body.into() }
    }
    fn error(status: u16, msg: &str) -> Self {
        Self::json(status, serde_json::json!({ "error": msg }).to_string())
    }
    fn to_bytes(&self) -> Vec<u8> {
        let reason = match self.status {
            200 => "OK",
            202 => "Accepted",
            400 => "Bad Request",
            401 => "Unauthorized",
            404 => "Not Found",
            405 => "Method Not Allowed",
            413 => "Payload Too Large",
            _ => "Error",
        };
        format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.status,
            reason,
            self.body.len(),
            self.body
        )
        .into_bytes()
    }
}

/// Route a fully-read request. Pure so it can be unit-tested.
fn route(
    head: &RequestHead,
    body: &[u8],
    token: Option<&str>,
) -> (Response, Vec<notchforge_core::model::NewNotification>) {
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        if !token_matches(t, head.authorization.as_deref()) {
            return (Response::error(401, "missing or invalid bearer token"), Vec::new());
        }
    }
    match (head.method.as_str(), head.path.trim_end_matches('/')) {
        ("GET", "/v1/health" | "/health") => (
            Response::json(200, serde_json::json!({"ok": true, "version": env!("CARGO_PKG_VERSION")}).to_string()),
            Vec::new(),
        ),
        ("POST", "/v1/notify" | "/notify") => {
            let get = |k: &str| head.query.iter().find(|(qk, _)| qk == k).map(|(_, v)| v.clone());
            let overrides = Overrides { source: get("source"), kind: get("kind") };
            match parse_events(body, Origin::AgentWebhook, &overrides) {
                Ok(events) => {
                    (Response::json(202, serde_json::json!({ "accepted": events.len() }).to_string()), events)
                }
                Err(e) => (Response::error(400, &e), Vec::new()),
            }
        }
        (_, "/v1/notify" | "/notify" | "/v1/health" | "/health") => {
            (Response::error(405, "method not allowed"), Vec::new())
        }
        _ => (Response::error(404, "not found"), Vec::new()),
    }
}

async fn read_request(stream: &mut TcpStream) -> Result<(RequestHead, Vec<u8>), Response> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_HEAD {
            return Err(Response::error(413, "headers too large"));
        }
        let n = stream.read(&mut chunk).await.map_err(|_| Response::error(400, "read error"))?;
        if n == 0 {
            return Err(Response::error(400, "connection closed"));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head_text = std::str::from_utf8(&buf[..head_end]).map_err(|_| Response::error(400, "non-UTF-8 headers"))?;
    let head = parse_head(head_text).map_err(|e| Response::error(400, e))?;
    if head.content_length > MAX_BODY {
        return Err(Response::error(413, "body too large"));
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < head.content_length {
        let n = stream.read(&mut chunk).await.map_err(|_| Response::error(400, "read error"))?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(head.content_length);
    Ok((head, body))
}

pub struct HttpAgentProvider;

impl Provider for HttpAgentProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "agents.http", name: "Agent webhook (HTTP)", permissions: &[] }
    }

    fn run(self: Box<Self>, ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            let cfg = ctx.config();
            let addr = ("127.0.0.1", cfg.agents.http_port);
            let listener = TcpListener::bind(addr)
                .await
                .map_err(|e| ProviderError::Unavailable(format!("cannot bind 127.0.0.1:{}: {e}", addr.1)))?;
            tracing::info!(port = addr.1, "agent webhook listening");
            ctx.set_status(ProviderStatus::Running).await;
            let mut shutdown = ctx.shutdown.clone();
            loop {
                let (mut stream, _) = tokio::select! {
                    r = listener.accept() => r?,
                    _ = shutdown.wait() => return Ok(()),
                };
                let events = ctx.events.clone();
                let token = ctx.config().agents.token.clone();
                tokio::spawn(async move {
                    let handled = tokio::time::timeout(IO_TIMEOUT, async {
                        let response = match read_request(&mut stream).await {
                            Ok((head, body)) => {
                                let (resp, notes) = route(&head, &body, token.as_deref());
                                for n in notes {
                                    let _ = events.send(ProviderEvent::Notification(n)).await;
                                }
                                resp
                            }
                            Err(resp) => resp,
                        };
                        let _ = stream.write_all(&response.to_bytes()).await;
                        let _ = stream.shutdown().await;
                    })
                    .await;
                    if handled.is_err() {
                        tracing::debug!("agent webhook connection timed out");
                    }
                });
                // Yield so a flood of connections cannot starve shutdown.
                if ctx.shutdown.is_triggered() {
                    return Ok(());
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_head_with_query_and_auth() {
        let h = parse_head(
            "POST /v1/notify?source=claude&kind=needs%20input HTTP/1.1\r\nHost: x\r\nContent-Length: 12\r\nAuthorization: Bearer s3cret",
        )
        .expect("parse");
        assert_eq!(h.method, "POST");
        assert_eq!(h.path, "/v1/notify");
        assert_eq!(h.query, vec![("source".into(), "claude".into()), ("kind".into(), "needs input".into())]);
        assert_eq!(h.content_length, 12);
        assert!(token_matches("s3cret", h.authorization.as_deref()));
        assert!(!token_matches("s3cres", h.authorization.as_deref()));
        assert!(!token_matches("s3cret", None));
    }

    #[test]
    fn rejects_bad_heads() {
        assert!(parse_head("GARBAGE").is_err());
        assert!(parse_head("POST / HTTP/2\r\n").is_err());
        assert!(parse_head("POST / HTTP/1.1\r\nTransfer-Encoding: chunked").is_err());
        assert!(parse_head("POST / HTTP/1.1\r\nContent-Length: abc").is_err());
    }

    #[test]
    fn percent_decoding_edge_cases() {
        assert_eq!(percent_decode("a%2Fb+c"), "a/b c");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn routing() {
        let head = |m: &str, p: &str| RequestHead {
            method: m.into(),
            path: p.into(),
            query: vec![],
            content_length: 0,
            authorization: None,
        };
        let (r, n) = route(&head("POST", "/v1/notify"), br#"{"title":"x"}"#, None);
        assert_eq!((r.status, n.len()), (202, 1));
        assert_eq!(route(&head("GET", "/v1/health"), b"", None).0.status, 200);
        assert_eq!(route(&head("GET", "/v1/notify"), b"", None).0.status, 405);
        assert_eq!(route(&head("GET", "/nope"), b"", None).0.status, 404);
        assert_eq!(route(&head("POST", "/v1/notify"), b"{", None).0.status, 400);
        assert_eq!(route(&head("POST", "/v1/notify"), br#"{"title":"x"}"#, Some("tok")).0.status, 401);
    }

    #[tokio::test]
    async fn end_to_end_over_tcp() {
        use notchforge_core::config::{Config, Paths};
        use notchforge_core::provider::{ProviderContext, shutdown_pair};
        use std::sync::Arc;
        use tokio::sync::{Notify, mpsc, watch};

        // Pick a free port.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            l.local_addr().expect("addr").port()
        };
        let mut cfg = Config::default();
        cfg.agents.http_port = port;
        let (tx, mut rx) = mpsc::channel(16);
        let (_cfg_tx, cfg_rx) = watch::channel(Arc::new(cfg));
        let (_loc_tx, loc_rx) = watch::channel(None);
        let (trigger, shutdown) = shutdown_pair();
        let ctx = ProviderContext {
            id: "agents.http",
            events: tx,
            config: cfg_rx,
            paths: Arc::new(Paths::rooted(&std::env::temp_dir())),
            refresh: Arc::new(Notify::new()),
            location: loc_rx,
            shutdown,
        };
        let task = tokio::spawn(Box::new(HttpAgentProvider).run(ctx));
        // Wait for "Running".
        loop {
            if let Some(ProviderEvent::Status { status: ProviderStatus::Running, .. }) = rx.recv().await {
                break;
            }
        }
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.expect("connect");
        let body = r#"{"source":"zed","kind":"error","title":"Build failed"}"#;
        let req = format!("POST /v1/notify HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        s.write_all(req.as_bytes()).await.expect("write");
        let mut resp = String::new();
        s.read_to_string(&mut resp).await.expect("read");
        assert!(resp.starts_with("HTTP/1.1 202"), "{resp}");
        match rx.recv().await {
            Some(ProviderEvent::Notification(n)) => assert_eq!(n.title, "Build failed"),
            other => panic!("unexpected {other:?}"),
        }
        trigger.trigger();
        task.await.expect("join").expect("provider ok");
    }
}
