//! `notchforge-mcp` — an MCP server (stdio, newline-delimited JSON-RPC 2.0)
//! that lets any MCP-capable coding agent post events to the Notchisland notch
//! through the app's loopback webhook (`POST 127.0.0.1:47815/v1/notify`).
//!
//! Environment: `NOTCHFORGE_PORT` (default 47815), `NOTCHFORGE_TOKEN`
//! (bearer token, if the webhook requires one). Logs go to stderr only —
//! stdout carries the protocol.

mod http;
mod server;

use std::io::{BufRead, Write};

fn main() {
    if std::env::args().skip(1).any(|a| a == "-h" || a == "--help") {
        println!(
            "notchforge-mcp — MCP stdio server for the Notchisland notch\n\n\
             Register it with your agent, e.g.\n    claude mcp add notchisland -- /Applications/Notchisland.app/Contents/MacOS/notchforge-mcp\n\n\
             Env: NOTCHFORGE_PORT (default 47815), NOTCHFORGE_TOKEN"
        );
        return;
    }
    let mut srv = server::Server::new(http::HttpSender::from_env());
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("notchforge-mcp: reading stdin: {e}");
                break;
            }
        };
        if let Some(resp) = srv.handle_line(&line) {
            let out = format!("{resp}\n");
            if stdout.write_all(out.as_bytes()).and_then(|()| stdout.flush()).is_err() {
                break;
            }
        }
    }
}
