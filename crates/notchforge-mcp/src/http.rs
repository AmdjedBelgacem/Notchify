//! Delivery of one event to the app's loopback webhook
//! (`POST http://127.0.0.1:<port>/v1/notify`), using only `std` networking —
//! the same approach as `notchforge-notify`.

use serde_json::Value;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Why an event could not be delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// Nothing is listening on the port: the app is not running.
    NotRunning,
    /// The app answered with a non-2xx status, or the exchange failed midway.
    Failed(String),
}

/// Anything that can deliver an event body. Injectable so the JSON-RPC layer
/// can be tested without a network.
pub trait Sender {
    fn send(&self, event: &Value) -> Result<(), SendError>;
}

/// Closures work as senders too (handy in tests).
impl<F: Fn(&Value) -> Result<(), SendError>> Sender for F {
    fn send(&self, event: &Value) -> Result<(), SendError> {
        self(event)
    }
}

/// The real sender: port from `$NOTCHFORGE_PORT` (default 47815), optional
/// bearer token from `$NOTCHFORGE_TOKEN`.
#[derive(Debug, Clone)]
pub struct HttpSender {
    pub port: u16,
    pub token: Option<String>,
}

impl HttpSender {
    pub fn from_env() -> Self {
        let port = std::env::var("NOTCHFORGE_PORT").ok().and_then(|p| p.trim().parse().ok()).unwrap_or(47815);
        let token = std::env::var("NOTCHFORGE_TOKEN").ok().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
        Self { port, token }
    }
}

impl Sender for HttpSender {
    fn send(&self, event: &Value) -> Result<(), SendError> {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.port));
        let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).map_err(|e| match e.kind() {
            ErrorKind::ConnectionRefused | ErrorKind::TimedOut | ErrorKind::AddrNotAvailable => SendError::NotRunning,
            _ => SendError::Failed(e.to_string()),
        })?;
        s.set_read_timeout(Some(Duration::from_secs(3))).ok();
        s.set_write_timeout(Some(Duration::from_secs(3))).ok();
        let body = event.to_string();
        let auth = self.token.as_deref().map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
        let req = format!(
            "POST /v1/notify HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).map_err(|e| SendError::Failed(e.to_string()))?;
        let mut resp = String::new();
        s.read_to_string(&mut resp).map_err(|e| SendError::Failed(e.to_string()))?;
        let status = resp.split_whitespace().nth(1).unwrap_or("");
        if status.starts_with('2') {
            Ok(())
        } else if status == "401" {
            Err(SendError::Failed("Notchisland rejected the bearer token (set NOTCHFORGE_TOKEN)".into()))
        } else {
            Err(SendError::Failed(format!("Notchisland replied {status}: {}", resp.lines().last().unwrap_or(""))))
        }
    }
}
