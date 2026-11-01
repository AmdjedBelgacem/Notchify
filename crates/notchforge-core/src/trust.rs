//! Remembered consent: which coding agents may show notifications in the
//! notch. Decisions are keyed by the app's rule key and stored as a small JSON
//! file next to the config.
//!
//! This is consent, not authentication: an agent is recognised by the source
//! name it sends, so the gate gives the user visibility and control over what
//! appears in the notch, and does not stop a local process from claiming an
//! agent's name (use the HTTP `token` for that).

use crate::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrustStore {
    decisions: BTreeMap<String, Decision>,
}

impl TrustStore {
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(CoreError::io(path, e)),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| CoreError::io(dir, e))?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?).map_err(|e| CoreError::io(path, e))
    }

    pub fn decision(&self, key: &str) -> Option<Decision> {
        self.decisions.get(key).copied()
    }

    pub fn set(&mut self, key: impl Into<String>, decision: Decision) {
        self.decisions.insert(key.into(), decision);
    }

    pub fn clear(&mut self) {
        self.decisions.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.decisions.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, Decision)> {
        self.decisions.iter().map(|(k, d)| (k.as_str(), *d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("nf-trust-{}-{}", std::process::id(), crate::time::now_ms()));
        let path = dir.join("trust.json");
        assert!(TrustStore::load(&path).expect("missing file is empty").is_empty());
        let mut t = TrustStore::default();
        t.set("claude", Decision::Allow);
        t.set("cursor", Decision::Deny);
        t.save(&path).expect("save");
        let back = TrustStore::load(&path).expect("load");
        assert_eq!(back.decision("claude"), Some(Decision::Allow));
        assert_eq!(back.decision("cursor"), Some(Decision::Deny));
        assert_eq!(back.decision("zed"), None);
        assert_eq!(back, t);
    }

    #[test]
    fn clear_forgets_everything() {
        let mut t = TrustStore::default();
        t.set("claude", Decision::Allow);
        t.clear();
        assert!(t.is_empty());
    }
}
