//! JSONL inbox file for coding agents.
//!
//! The simplest possible integration: append one JSON event per line to
//! `<data dir>/inbox.jsonl` from any script, hook or editor task:
//!
//! ```sh
//! echo '{"source":"zed","kind":"task_finished","title":"Tests passed"}' >> ~/Library/Application\ Support/NotchForge/inbox.jsonl
//! ```
//!
//! The provider tails the file (starting at its end, so old lines are never
//! replayed), handles truncation/rotation, and buffers partial lines.

use super::payload::{Overrides, parse_events};
use notchforge_core::model::{NewNotification, Origin, ProviderStatus};
use notchforge_core::provider::{BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent, Wake};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

const POLL: Duration = Duration::from_millis(750);
/// The inbox is truncated once it exceeds this size and has been fully read.
const ROTATE_AT: u64 = 1024 * 1024;

/// Incremental reader state.
#[derive(Debug, Default)]
pub struct Tail {
    offset: u64,
    partial: Vec<u8>,
    inode: Option<u64>,
}

impl Tail {
    /// Start at the current end of `path` (or 0 if it does not exist).
    pub fn at_end(path: &Path) -> Self {
        let meta = std::fs::metadata(path).ok();
        Self { offset: meta.as_ref().map_or(0, |m| m.len()), partial: Vec::new(), inode: meta.as_ref().and_then(inode) }
    }

    /// Read newly appended complete lines.
    pub fn poll(&mut self, path: &Path) -> std::io::Result<Vec<String>> {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.offset = 0;
                self.partial.clear();
                return Ok(Vec::new());
            }
            Err(e) => return Err(e),
        };
        let ino = inode(&meta);
        // Truncated or replaced: start over from the beginning of the new file.
        if meta.len() < self.offset || (ino.is_some() && self.inode.is_some() && ino != self.inode) {
            self.offset = 0;
            self.partial.clear();
        }
        self.inode = ino;
        if meta.len() == self.offset {
            return Ok(Vec::new());
        }
        let mut f = std::fs::File::open(path)?;
        f.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::new();
        f.take(4 * 1024 * 1024).read_to_end(&mut buf)?;
        self.offset += buf.len() as u64;
        self.partial.extend_from_slice(&buf);
        let mut lines = Vec::new();
        while let Some(pos) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=pos).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]).trim().to_owned();
            if !text.is_empty() {
                lines.push(text);
            }
        }
        Ok(lines)
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }
}

#[cfg(unix)]
fn inode(m: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(m.ino())
}

#[cfg(not(unix))]
fn inode(_: &std::fs::Metadata) -> Option<u64> {
    None
}

pub fn parse_lines(lines: &[String]) -> Vec<NewNotification> {
    lines
        .iter()
        .flat_map(|l| match parse_events(l.as_bytes(), Origin::AgentInbox, &Overrides::default()) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "ignoring malformed inbox line");
                Vec::new()
            }
        })
        .collect()
}

pub struct InboxAgentProvider;

impl Provider for InboxAgentProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "agents.inbox", name: "Agent inbox file", permissions: &[] }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            let path = ctx.paths.inbox_path(&ctx.config().agents);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if !path.exists() {
                std::fs::write(&path, b"")?;
            }
            let mut tail = Tail::at_end(&path);
            tracing::info!(path = %path.display(), "agent inbox watching");
            ctx.set_status(ProviderStatus::Running).await;
            loop {
                for n in parse_lines(&tail.poll(&path)?) {
                    if !ctx.emit(ProviderEvent::Notification(n)).await {
                        return Ok(());
                    }
                }
                // Keep the file small: once everything is consumed, truncate.
                if tail.offset() > ROTATE_AT {
                    std::fs::write(&path, b"")?;
                    tail = Tail::at_end(&path);
                }
                if ctx.sleep(POLL).await == Wake::Shutdown {
                    return Ok(());
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn tails_appends_partial_lines_and_truncation() {
        let path = std::env::temp_dir().join(format!("nf-inbox-{}.jsonl", std::process::id()));
        std::fs::write(&path, "{\"title\":\"old\"}\n").expect("write");
        let mut tail = Tail::at_end(&path);
        assert!(tail.poll(&path).expect("poll").is_empty());

        let mut f = std::fs::OpenOptions::new().append(true).open(&path).expect("open");
        write!(f, "{{\"title\":\"a\"}}\n{{\"title\":").expect("write");
        f.flush().expect("flush");
        let lines = tail.poll(&path).expect("poll");
        assert_eq!(lines, vec!["{\"title\":\"a\"}".to_owned()]);
        writeln!(f, "\"b\"}}").expect("write");
        f.flush().expect("flush");
        let notes = parse_lines(&tail.poll(&path).expect("poll"));
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "b");

        std::fs::write(&path, "{\"title\":\"after-truncate\"}\n").expect("truncate");
        let notes = parse_lines(&tail.poll(&path).expect("poll"));
        assert_eq!(notes[0].title, "after-truncate");

        std::fs::remove_file(&path).expect("rm");
        assert!(tail.poll(&path).expect("poll").is_empty());
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let notes = parse_lines(&["nope".into(), "{\"title\":\"ok\"}".into()]);
        assert_eq!(notes.len(), 1);
    }
}
