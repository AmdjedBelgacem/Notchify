//! Unix-domain socket listener for coding agents.
//!
//! Each line written to `<data dir>/agent.sock` is one JSON event (same schema
//! as the HTTP webhook). The server replies `ok` or `error: …` per line. The
//! socket file is created with mode `0600`, so only the current user can post.
//!
//! ```sh
//! echo '{"source":"opencode","kind":"task_finished","title":"Done"}' | nc -U ~/Library/Application\ Support/NotchForge/agent.sock
//! ```

use super::payload::{Overrides, parse_events};
use notchforge_core::model::{Origin, ProviderStatus};
use notchforge_core::provider::{BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent};

pub struct SocketAgentProvider;

/// Longest accepted line; longer lines are rejected to bound memory.
pub const MAX_LINE: usize = 256 * 1024;

impl Provider for SocketAgentProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "agents.socket", name: "Agent socket", permissions: &[] }
    }

    #[cfg(unix)]
    fn run(self: Box<Self>, ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            use std::os::unix::fs::PermissionsExt;
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            use tokio::net::UnixListener;

            let path = ctx.paths.socket_path(&ctx.config().agents);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // A stale socket from a crashed run would make bind fail.
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
            let listener = UnixListener::bind(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            tracing::info!(path = %path.display(), "agent socket listening");
            ctx.set_status(ProviderStatus::Running).await;

            let mut shutdown = ctx.shutdown.clone();
            let result = loop {
                let (stream, _) = tokio::select! {
                    r = listener.accept() => match r { Ok(s) => s, Err(e) => break Err(e.into()) },
                    _ = shutdown.wait() => break Ok(()),
                };
                let events = ctx.events.clone();
                let mut conn_shutdown = ctx.shutdown.clone();
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut lines = BufReader::new(read).lines();
                    loop {
                        let line = tokio::select! {
                            l = lines.next_line() => l,
                            _ = conn_shutdown.wait() => break,
                        };
                        let line = match line {
                            Ok(Some(l)) => l,
                            _ => break,
                        };
                        if line.trim().is_empty() {
                            continue;
                        }
                        let reply = if line.len() > MAX_LINE {
                            "error: line too long\n".to_owned()
                        } else {
                            match parse_events(line.as_bytes(), Origin::AgentSocket, &Overrides::default()) {
                                Ok(notes) => {
                                    for n in notes {
                                        let _ = events.send(ProviderEvent::Notification(n)).await;
                                    }
                                    "ok\n".to_owned()
                                }
                                Err(e) => format!("error: {e}\n"),
                            }
                        };
                        if write.write_all(reply.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                });
            };
            let _ = std::fs::remove_file(&path);
            result
        })
    }

    #[cfg(not(unix))]
    fn run(self: Box<Self>, ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            ctx.set_status(ProviderStatus::Failing("Unix sockets unsupported".into())).await;
            Ok(())
        })
    }
}
