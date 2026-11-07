//! GitHub Actions runs as live activities.
//!
//! Through the `gh` CLI (already signed in, so no token is stored here): the
//! repositories in `[ci] repos`, plus the ones you pushed to in the last few
//! hours, are checked for workflow runs. A run that starts peeks in the notch;
//! when it finishes, a notification says how it went with a link to it.
//! Nothing happens without `gh` or without a sign-in.

use notchforge_core::apps::AppRef;
use notchforge_core::model::{NewNotification, NotificationKind, Origin, Priority, ProviderStatus};
use notchforge_core::provider::{
    BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Default)]
pub struct CiProvider;

/// How long a push keeps its repository watched.
const RECENT_PUSH_MS: i64 = 6 * 3600 * 1000;
/// Quiet polling when nothing is running.
const IDLE_POLL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub database_id: u64,
    pub status: String,
    #[serde(default)]
    pub conclusion: String,
    pub name: String,
    #[serde(default)]
    pub display_title: String,
    #[serde(default)]
    pub head_branch: String,
    pub url: String,
    #[serde(default)]
    pub created_at: String,
}

impl Run {
    pub fn is_done(&self) -> bool {
        self.status == "completed"
    }
}

/// What a run's change of state should say, if anything.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Started,
    Finished,
}

/// Compare a poll with what was seen before. The first sight of a repository
/// announces nothing (no flood of old runs), except runs still going.
pub fn changes(seen: &HashMap<u64, String>, runs: &[Run], first_look: bool) -> Vec<(Run, Change)> {
    let mut out = Vec::new();
    for r in runs {
        match seen.get(&r.database_id) {
            None if !r.is_done() => out.push((r.clone(), Change::Started)),
            None if first_look => {}
            None => out.push((r.clone(), Change::Finished)), // started and finished between polls
            Some(prev) if prev != "completed" && r.is_done() => out.push((r.clone(), Change::Finished)),
            Some(_) => {}
        }
    }
    out
}

pub fn notification(repo: &str, run: &Run, change: &Change) -> NewNotification {
    let short = repo.rsplit('/').next().unwrap_or(repo);
    let (kind, priority, title) = match (change, run.conclusion.as_str()) {
        (Change::Started, _) => (NotificationKind::Info, Priority::Low, format!("{} running · {short}", run.name)),
        (Change::Finished, "success") => {
            (NotificationKind::TaskFinished, Priority::Normal, format!("{} passed · {short}", run.name))
        }
        (Change::Finished, "cancelled" | "skipped") => {
            (NotificationKind::Info, Priority::Low, format!("{} {} · {short}", run.name, run.conclusion))
        }
        (Change::Finished, _) => (NotificationKind::Error, Priority::High, format!("{} failed · {short}", run.name)),
    };
    let body = if run.head_branch.is_empty() {
        run.display_title.clone()
    } else {
        format!("{} — {}", run.head_branch, run.display_title)
    };
    NewNotification {
        app: AppRef { known: None, bundle_id: None, display_name: "GitHub Actions".into(), platform: None },
        origin: Origin::Internal,
        kind,
        priority: Some(priority),
        title,
        subtitle: None,
        body,
        timestamp_ms: notchforge_core::time::now_ms(),
        thread: Some(format!("ci:{}", run.database_id)),
        url: Some(run.url.clone()),
        tokens: None,
    }
}

/// `gh`, where Homebrew or the installer put it (apps don't get a shell PATH).
fn gh_path() -> Option<std::path::PathBuf> {
    ["/opt/homebrew/bin/gh", "/usr/local/bin/gh", "/usr/bin/gh"]
        .iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.exists())
}

async fn gh(bin: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(bin).args(args).kill_on_drop(true).output(),
    )
    .await
    .ok()?
    .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Repositories from your recent push events, newest first.
pub fn pushed_repos(events_json: &str, now_ms: i64) -> Vec<String> {
    #[derive(Deserialize)]
    struct Repo {
        name: String,
    }
    #[derive(Deserialize)]
    struct Event {
        #[serde(rename = "type")]
        kind: String,
        repo: Repo,
        created_at: String,
    }
    let events: Vec<Event> = serde_json::from_str(events_json).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for e in events {
        if e.kind != "PushEvent" {
            continue;
        }
        let at = crate::agents::usage::parse_time(&e.created_at).unwrap_or(0);
        if now_ms - at > RECENT_PUSH_MS {
            continue;
        }
        if !out.contains(&e.repo.name) {
            out.push(e.repo.name);
        }
    }
    out.truncate(5);
    out
}

impl Provider for CiProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "ci.github", name: "GitHub Actions runs", permissions: &[] }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            let Some(bin) = gh_path() else {
                ctx.set_status(ProviderStatus::Failing("The GitHub CLI (gh) is not installed".into())).await;
                ctx.shutdown.wait().await;
                return Ok(());
            };
            // Whose pushes to follow: looked up when needed, retried while the
            // network is down. Signed out is decided by `gh auth status` alone.
            let mut login: Option<String> = None;
            if gh(&bin, &["auth", "status"]).await.is_none() {
                ctx.set_status(ProviderStatus::Failing("Sign in with `gh auth login` to see CI runs".into())).await;
            }
            let mut healthy = false;
            let mut seen: HashMap<String, HashMap<u64, String>> = HashMap::new();
            let mut pushed: (Vec<String>, i64) = (Vec::new(), 0);
            loop {
                let cfg = ctx.config();
                let now = notchforge_core::time::now_ms();
                if cfg.ci.follow_my_pushes && login.is_none() {
                    login = gh(&bin, &["api", "user", "--jq", ".login"]).await.map(|s| s.trim().to_owned());
                }
                if let (true, Some(me)) = (cfg.ci.follow_my_pushes && now - pushed.1 > 5 * 60_000, login.as_deref()) {
                    if let Some(json) = gh(&bin, &["api", &format!("users/{me}/events?per_page=40")]).await {
                        pushed = (pushed_repos(&json, now), now);
                    }
                }
                let mut repos = cfg.ci.repos.clone();
                for r in &pushed.0 {
                    if !repos.contains(r) {
                        repos.push(r.clone());
                    }
                }
                let mut running = false;
                for repo in &repos {
                    let fields = "databaseId,status,conclusion,name,displayTitle,headBranch,url,createdAt";
                    let Some(json) = gh(&bin, &["run", "list", "-R", repo, "--limit", "8", "--json", fields]).await
                    else {
                        tracing::debug!(repo = %repo, "could not list CI runs (offline?)");
                        continue;
                    };
                    if !healthy {
                        ctx.set_status(ProviderStatus::Running).await;
                        healthy = true;
                    }
                    let runs: Vec<Run> = serde_json::from_str(&json).unwrap_or_default();
                    let first = !seen.contains_key(repo);
                    let known = seen.entry(repo.clone()).or_default();
                    for (run, change) in changes(known, &runs, first) {
                        tracing::info!(repo = %repo, run = %run.name, ?change, "CI run");
                        if !ctx.emit(ProviderEvent::Notification(notification(repo, &run, &change))).await {
                            return Ok(());
                        }
                    }
                    for r in &runs {
                        known.insert(r.database_id, r.status.clone());
                    }
                    running |= runs.iter().any(|r| !r.is_done());
                }
                let wait = if running { Duration::from_secs(cfg.ci.poll_seconds.clamp(15, 600)) } else { IDLE_POLL };
                let mut shutdown = ctx.shutdown.clone();
                tokio::select! {
                    _ = shutdown.wait() => return Ok(()),
                    _ = tokio::time::sleep(wait) => {}
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: u64, status: &str, conclusion: &str) -> Run {
        Run {
            database_id: id,
            status: status.into(),
            conclusion: conclusion.into(),
            name: "CI".into(),
            display_title: "Fix the player".into(),
            head_branch: "main".into(),
            url: format!("https://github.com/o/r/actions/runs/{id}"),
            created_at: String::new(),
        }
    }

    #[test]
    fn runs_announce_start_and_end_once() {
        let mut seen = HashMap::new();
        // First look: old finished runs stay quiet, a running one is announced.
        let first = changes(&seen, &[run(1, "completed", "success"), run(2, "in_progress", "")], true);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].1, Change::Started);
        seen.insert(1, "completed".to_owned());
        seen.insert(2, "in_progress".to_owned());
        let next = changes(&seen, &[run(1, "completed", "success"), run(2, "completed", "failure")], false);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].1, Change::Finished);
        let n = notification("o/r", &next[0].0, &next[0].1);
        assert_eq!(n.title, "CI failed · r");
        assert_eq!(n.kind, NotificationKind::Error);
        assert_eq!(n.body, "main — Fix the player");
        // Started and finished between two polls: still said once.
        assert_eq!(changes(&seen, &[run(3, "completed", "success")], false).len(), 1);
    }

    #[test]
    fn recent_pushes_pick_the_repositories() {
        let now = crate::agents::usage::parse_time("2026-11-07T12:00:00Z").expect("time");
        let json = r#"[
            {"type":"PushEvent","repo":{"name":"o/a"},"created_at":"2026-11-07T11:00:00Z"},
            {"type":"WatchEvent","repo":{"name":"o/b"},"created_at":"2026-11-07T11:30:00Z"},
            {"type":"PushEvent","repo":{"name":"o/a"},"created_at":"2026-11-07T10:00:00Z"},
            {"type":"PushEvent","repo":{"name":"o/old"},"created_at":"2026-11-06T01:00:00Z"}
        ]"#;
        assert_eq!(pushed_repos(json, now), vec!["o/a".to_owned()]);
        assert!(pushed_repos("not json", now).is_empty());
    }
}
