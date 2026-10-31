//! **Private Apple APIs.** Everything in here may break with any macOS update.
//! Each entry point is resolved at runtime with `dlopen`/`dlsym`; when a
//! symbol is missing the feature is reported as unavailable and the UI hides
//! the control — NotchForge never crashes because of these.

pub mod display_services;
