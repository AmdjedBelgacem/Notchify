//! Error type shared by the core crate.

use std::path::PathBuf;

/// Errors produced by core logic (mostly configuration and persistence).
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("failed to parse configuration: {0}")]
    ConfigParse(#[from] toml::de::Error),
    #[error("failed to serialize configuration: {0}")]
    ConfigSerialize(#[from] toml::ser::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no suitable directory for {0}")]
    NoDirectory(&'static str),
}

impl CoreError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }
}

pub type Result<T, E = CoreError> = std::result::Result<T, E>;
