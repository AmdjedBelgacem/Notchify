//! Zero-setup "agent finished" notifications.
//!
//! Hooks (see [`super::connect`]) report every finished *turn*. This watcher
//! covers the other end without any setup: when a coding-agent CLI process
//! that has been running for a while exits, the notch says that agent ended
//! its session. It reads `ps` every few seconds, which is cheap and needs no
//! permission.

use notchforge_core::apps::{AppRef, KnownApp};
use notchforge_core::model::{NewNotification, NotificationKind, Origin, ProviderStatus};
use notchforge_core::provider::{
    BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent, Wake,
};
use std::collections::HashMap;
use std::time::Duration;

pub struct AgentWatchProvider;

const POLL: Duration = Duration::from_secs(4);
/// Sessions shorter than this are not worth announcing (`claude --version`…).
const MIN_RUNTIME_SECS: u64 = 20;

/// CLI names that identify an agent: (executable name, source key, display name).
const AGENTS: &[(&str, &str, &str)] = &[
    ("claude", "claude", "Claude Code"),
    ("codex", "chatgpt", "Codex"),
    ("opencode", "opencode", "OpenCode"),
    ("cursor-agent", "cursor", "Cursor Agent"),
    ("qodercli", "qoder", "Qoder CLI"),
    ("gemini", "agent", "Gemini CLI"),
    ("aider", "agent", "Aider"),
    ("goose", "agent", "Goose"),
    ("amp", "agent", "Amp"),
    ("copilot", "agent", "Copilot CLI"),
];

/// One running agent process.
#[derive(Debug, Clone, PartialEq)]
pub struct Running {
    pub pid: i32,
    pub key: &'static str,
    pub name: &'static str,
    pub seconds: u64,
}

/// `[[dd-]hh:]mm:ss` → seconds.
pub fn parse_etime(s: &str) -> Option<u64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let parts: Vec<u64> = rest.split(':').map(|p| p.parse::<u64>().ok()).collect::<Option<_>>()?;
    let secs = match parts[..] {
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some(days * 86_400 + secs)
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Recognise an agent from a process's command line: its executable, or the
/// script a `node` launcher runs. Desktop apps (inside `.app` bundles) and
/// helper subcommands are not sessions.
pub fn classify(args: &str) -> Option<(&'static str, &'static str)> {
    let mut it = args.split_whitespace();
    let argv0 = it.next()?;
    let target =
        if matches!(basename(argv0), "node" | "bun" | "deno") { it.find(|a| !a.starts_with('-'))? } else { argv0 };
    if target.contains(".app/Contents/") && !target.contains("claude") && !target.contains("codex") {
        return None;
    }
    let exe = basename(target);
    AGENTS.iter().find(|(n, _, _)| *n == exe).map(|(_, k, d)| (*k, *d))
}

/// Parse `ps -axo pid=,etime=,args=` output.
pub fn parse_ps(out: &str) -> Vec<Running> {
    out.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let (pid, rest) = l.split_once(char::is_whitespace)?;
            let rest = rest.trim_start();
            let (etime, args) = rest.split_once(char::is_whitespace)?;
            let (key, name) = classify(args.trim_start())?;
            Some(Running { pid: pid.parse().ok()?, key, name, seconds: parse_etime(etime)? })
        })
        .collect()
}

fn ps() -> Vec<Running> {
    std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,etime=,args="])
        .output()
        .map(|o| parse_ps(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// Sessions that were in `before` and are gone from `now`, long enough to matter.
pub fn ended<'a>(before: &'a HashMap<i32, Running>, now: &[Running]) -> Vec<&'a Running> {
    let mut gone: Vec<&Running> =
        before.values().filter(|r| !now.iter().any(|n| n.pid == r.pid) && r.seconds >= MIN_RUNTIME_SECS).collect();
    gone.sort_by_key(|r| r.pid);
    gone
}

fn notification(r: &Running) -> NewNotification {
    let app = match KnownApp::from_key(r.key) {
        Some(k) if k != KnownApp::GenericAgent => AppRef::known(k),
        _ => AppRef { known: Some(KnownApp::GenericAgent), bundle_id: None, display_name: r.name.to_owned() },
    };
    let mins = r.seconds / 60;
    let took = if mins >= 1 { format!("after {mins} min") } else { format!("after {} s", r.seconds) };
    NewNotification {
        app,
        origin: Origin::Internal,
        kind: NotificationKind::TaskFinished,
        priority: None,
        title: format!("{} finished", r.name),
        subtitle: None,
        body: format!("The session ended {took}."),
        timestamp_ms: notchforge_core::time::now_ms(),
        thread: Some(format!("agent-session-{}", r.pid)),
        url: None,
    }
}

impl Provider for AgentWatchProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "agent-watch", name: "Agent sessions", permissions: &[] }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            if !cfg!(target_os = "macos") {
                ctx.set_status(ProviderStatus::Failing("Agent sessions are only watched on macOS".into())).await;
                ctx.shutdown.wait().await;
                return Ok(());
            }
            let mut known: HashMap<i32, Running> = HashMap::new();
            ctx.set_status(ProviderStatus::Running).await;
            loop {
                let now = tokio::task::spawn_blocking(ps).await.unwrap_or_default();
                for r in ended(&known, &now) {
                    if !ctx.emit(ProviderEvent::Notification(notification(r))).await {
                        return Ok(());
                    }
                }
                known = now.into_iter().map(|r| (r.pid, r)).collect();
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

    #[test]
    fn parses_elapsed_times() {
        assert_eq!(parse_etime("00:07"), Some(7));
        assert_eq!(parse_etime("12:34"), Some(754));
        assert_eq!(parse_etime("1:02:03"), Some(3723));
        assert_eq!(parse_etime("2-00:00:01"), Some(172_801));
        assert_eq!(parse_etime("nope"), None);
    }

    #[test]
    fn recognises_agent_clis() {
        assert_eq!(classify("/Users/me/.local/bin/claude --resume").map(|a| a.0), Some("claude"));
        assert_eq!(classify("node /opt/homebrew/bin/gemini").map(|a| a.1), Some("Gemini CLI"));
        assert_eq!(classify("/opt/homebrew/bin/codex exec fix").map(|a| a.0), Some("chatgpt"));
        assert_eq!(classify("/Applications/Cursor.app/Contents/MacOS/Cursor"), None);
        assert_eq!(classify("/bin/zsh -l"), None);
        assert_eq!(classify("node server.js"), None);
    }

    #[test]
    fn announces_only_long_sessions_that_ended() {
        let out = "  101  05:00 /usr/local/bin/claude\n  102  00:05 /usr/local/bin/codex\n  103  10:00 /bin/zsh\n";
        let before: HashMap<i32, Running> = parse_ps(out).into_iter().map(|r| (r.pid, r)).collect();
        assert_eq!(before.len(), 2);
        let now = parse_ps("  102  00:09 /usr/local/bin/codex\n");
        let gone = ended(&before, &now);
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].name, "Claude Code");
        let n = notification(gone[0]);
        assert_eq!(n.kind, NotificationKind::TaskFinished);
        assert_eq!(n.title, "Claude Code finished");
        assert!(n.body.contains("5 min"));
    }
}
