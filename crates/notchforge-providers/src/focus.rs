//! Focus / Do Not Disturb state.
//!
//! **Fragile — isolated here.** macOS has no public API for reading the Focus
//! state. The Focus daemon persists active assertions in
//! `~/Library/DoNotDisturb/DB/Assertions.json` and mode names in
//! `ModeConfigurations.json` (both need Full Disk Access). A non-empty
//! `storeAssertionRecords` list means a Focus is active. Schedule-based Focus
//! that has no assertion is not visible this way (documented limitation).
//!
//! Toggling is done by the macOS layer through user-provided Shortcuts.

use notchforge_core::model::{FocusState, Permission, ProviderStatus};
use notchforge_core::provider::{BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent, Wake};
use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const POLL: Duration = Duration::from_secs(3);

fn db_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join("Library/DoNotDisturb/DB"))
}

/// Returns the active mode identifier, if any.
pub fn parse_assertions(json: &str) -> Option<Option<String>> {
    let v: Value = serde_json::from_str(json).ok()?;
    let records = v.get("data")?.as_array()?.iter().filter_map(|d| d.get("storeAssertionRecords")?.as_array()).flatten();
    let mut active = false;
    let mut mode = None;
    for r in records {
        active = true;
        if mode.is_none() {
            mode = r
                .pointer("/assertionDetails/assertionDetailsModeIdentifier")
                .and_then(|m| m.as_str())
                .map(str::to_owned);
        }
    }
    Some(if active { Some(mode.unwrap_or_default()) } else { None })
}

/// Look up a human name for a mode identifier.
pub fn parse_mode_name(json: &str, identifier: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;
    v.get("data")?.as_array()?.iter().find_map(|d| {
        d.get("modeConfigurations")?.get(identifier)?.pointer("/mode/name")?.as_str().map(str::to_owned)
    })
}

fn read_state() -> Result<FocusState, ProviderError> {
    let dir = db_dir().ok_or_else(|| ProviderError::Unavailable("no home directory".into()))?;
    let text = match std::fs::read_to_string(dir.join("Assertions.json")) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(ProviderError::NeedsPermission(Permission::FullDiskAccess));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FocusState { active: Some(false), mode_name: None }),
        Err(e) => return Err(e.into()),
    };
    let Some(active) = parse_assertions(&text) else {
        return Err(ProviderError::Unavailable("unrecognised Focus database format".into()));
    };
    let mode_name = active.as_deref().and_then(|id| {
        let cfg = std::fs::read_to_string(dir.join("ModeConfigurations.json")).ok()?;
        parse_mode_name(&cfg, id)
    });
    Ok(FocusState { active: Some(active.is_some()), mode_name })
}

pub struct FocusProvider;

impl Provider for FocusProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "focus", name: "Focus state", permissions: &[Permission::FullDiskAccess] }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            if !cfg!(target_os = "macos") {
                ctx.set_status(ProviderStatus::Failing("Focus state needs macOS".into())).await;
                return Ok(());
            }
            let file = db_dir().map(|d| d.join("Assertions.json"));
            let mtime = || -> Option<SystemTime> { std::fs::metadata(file.as_ref()?).ok()?.modified().ok() };
            let mut last_mtime = None;
            let mut first = true;
            loop {
                let m = mtime();
                if first || m != last_mtime {
                    first = false;
                    last_mtime = m;
                    let state = tokio::task::spawn_blocking(read_state).await.map_err(ProviderError::other)??;
                    ctx.set_status(ProviderStatus::Running).await;
                    if !ctx.emit(ProviderEvent::Focus(state)).await {
                        return Ok(());
                    }
                }
                if ctx.sleep_or_refresh(POLL).await == Wake::Shutdown {
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
    fn assertions() {
        let active = r#"{"data":[{"storeAssertionRecords":[{"assertionDetails":{"assertionDetailsModeIdentifier":"com.apple.focus.work"}}]}]}"#;
        assert_eq!(parse_assertions(active), Some(Some("com.apple.focus.work".into())));
        let idle = r#"{"data":[{"storeAssertionRecords":[]}]}"#;
        assert_eq!(parse_assertions(idle), Some(None));
        assert_eq!(parse_assertions(r#"{"data":[{}]}"#), Some(None));
        assert_eq!(parse_assertions("nope"), None);
    }

    #[test]
    fn mode_names() {
        let cfg = r#"{"data":[{"modeConfigurations":{"com.apple.focus.work":{"mode":{"name":"Work"}}}}]}"#;
        assert_eq!(parse_mode_name(cfg, "com.apple.focus.work"), Some("Work".into()));
        assert_eq!(parse_mode_name(cfg, "other"), None);
    }
}
