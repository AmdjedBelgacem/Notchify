//! "A new version is out": the latest GitHub release's `appcast.json`
//! (written by the release workflow), checked twice a day. A newer version is
//! announced once, with a link to download it; nothing is installed for you.
//! Homebrew users upgrade with `brew upgrade --cask notchisland`.

use notchforge_core::apps::AppRef;
use notchforge_core::model::{NewNotification, NotificationKind, Origin, Priority, ProviderStatus};
use notchforge_core::provider::{
    BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent,
};
use serde::Deserialize;
use std::time::Duration;

pub const APPCAST_URL: &str = "https://github.com/AmdjedBelgacem/Notchify/releases/latest/download/appcast.json";
const RELEASES_URL: &str = "https://github.com/AmdjedBelgacem/Notchify/releases/latest";
const CHECK_EVERY: Duration = Duration::from_secs(12 * 3600);
const SEEN_FILE: &str = "update-announced.txt";

#[derive(Default)]
pub struct UpdateProvider;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Appcast {
    pub version: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub notes: String,
}

/// `1.2.10` > `1.2.9`; a pre-release (`1.3.0-beta`) never beats its release.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    fn parts(v: &str) -> (Vec<u64>, bool) {
        let v = v.trim().trim_start_matches('v');
        let (core, pre) = match v.split_once('-') {
            Some((c, _)) => (c, true),
            None => (v, false),
        };
        (core.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect(), pre)
    }
    let (mut a, a_pre) = parts(candidate);
    let (mut b, b_pre) = parts(current);
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    match a.cmp(&b) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => b_pre && !a_pre,
    }
}

pub fn notification(cast: &Appcast) -> NewNotification {
    let notes = cast.notes.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    NewNotification {
        app: AppRef { known: None, bundle_id: None, display_name: "Notchisland".into(), platform: None },
        origin: Origin::Internal,
        kind: NotificationKind::Info,
        priority: Some(Priority::Normal),
        title: format!("Notchisland {} is available", cast.version.trim_start_matches('v')),
        subtitle: None,
        body: if notes.is_empty() { "Open to download it.".into() } else { notes.chars().take(200).collect() },
        timestamp_ms: notchforge_core::time::now_ms(),
        thread: Some("update".into()),
        url: Some(RELEASES_URL.into()),
        tokens: None,
    }
}

impl Provider for UpdateProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "update", name: "Update check", permissions: &[] }
    }

    fn run(self: Box<Self>, ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            ctx.set_status(ProviderStatus::Running).await;
            let http = reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent(concat!("Notchisland/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(ProviderError::other)?;
            let seen_path = ctx.paths.data_dir.join(SEEN_FILE);
            // Not right at launch: the network may not be up yet.
            let mut wait = Duration::from_secs(90);
            loop {
                let mut shutdown = ctx.shutdown.clone();
                tokio::select! {
                    _ = shutdown.wait() => return Ok(()),
                    _ = tokio::time::sleep(wait) => {}
                }
                wait = CHECK_EVERY;
                let Ok(resp) = http.get(APPCAST_URL).send().await else { continue };
                if !resp.status().is_success() {
                    continue; // no release yet
                }
                let Ok(cast) = resp.json::<Appcast>().await else { continue };
                let announced = std::fs::read_to_string(&seen_path).unwrap_or_default();
                if is_newer(&cast.version, env!("CARGO_PKG_VERSION")) && announced.trim() != cast.version {
                    tracing::info!(version = %cast.version, "a newer release is available");
                    if !ctx.emit(ProviderEvent::Notification(notification(&cast))).await {
                        return Ok(());
                    }
                    let _ = std::fs::write(&seen_path, &cast.version);
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v1.2.10", "1.2.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.2.0-beta", "0.2.0"));
        assert!(is_newer("0.2.0", "0.2.0-beta"));
        assert!(is_newer("1.0", "0.9.9"));
    }

    #[test]
    fn announces_with_the_first_line_of_the_notes() {
        let n = notification(&Appcast {
            version: "v0.2.0".into(),
            url: String::new(),
            notes: "\nFaster hooks\nMore".into(),
        });
        assert_eq!(n.title, "Notchisland 0.2.0 is available");
        assert_eq!(n.body, "Faster hooks");
    }
}
