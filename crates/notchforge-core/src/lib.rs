//! # notchforge-core
//!
//! Everything in NotchForge that does not need to talk to the operating system:
//!
//! * [`model`] / [`apps`] — the data that flows through the app,
//! * [`config`] — TOML configuration with defaults and validation,
//! * [`geometry`] — notch detection math and layout rectangles,
//! * [`spring`] — the damped-spring animation primitive,
//! * [`machine`] — the pure UI state machine (collapsed / peek / hover / expanded),
//! * [`pipeline`] — notification filtering, dedupe, rate limiting, grouping, ranking,
//! * [`tray`] — the temporary file tray,
//! * [`provider`] — the [`Provider`](provider::Provider) trait and supervisor,
//! * [`engine`] — the single owner of mutable application state.
//!
//! The crate compiles and is fully tested on any OS; the macOS front-end lives in
//! `notchforge-macos` and only consumes [`engine::Snapshot`]s and emits
//! [`engine::Command`]s.

pub mod apps;
pub mod config;
pub mod engine;
pub mod error;
pub mod geometry;
pub mod machine;
pub mod model;
pub mod pipeline;
pub mod provider;
pub mod spring;
pub mod time;
pub mod tray;

pub use error::{CoreError, Result};
