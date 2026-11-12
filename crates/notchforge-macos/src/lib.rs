//! # notchforge-macos
//!
//! The AppKit front-end. Compiles to an empty crate on other platforms.

#[cfg(target_os = "macos")]
pub mod accessibility;
#[cfg(target_os = "macos")]
pub mod app;
#[cfg(target_os = "macos")]
pub mod bridge;
#[cfg(target_os = "macos")]
pub mod call_watch;
/// Screenshots, screen recording and voice memos from the notch.
pub mod capture;
#[cfg(target_os = "macos")]
pub mod controller;
#[cfg(target_os = "macos")]
pub mod controls;
#[cfg(target_os = "macos")]
pub mod ffi;
#[cfg(target_os = "macos")]
pub mod glass;
#[cfg(target_os = "macos")]
pub mod hotkey;
/// Interface language (English, French, Arabic).
pub mod i18n;
#[cfg(target_os = "macos")]
pub mod icons;
#[cfg(target_os = "macos")]
pub mod location;
#[cfg(target_os = "macos")]
pub mod login_item;
#[cfg(target_os = "macos")]
pub mod media;
#[cfg(target_os = "macos")]
pub mod menubar;
#[cfg(target_os = "macos")]
pub mod mirrors;
#[cfg(target_os = "macos")]
pub mod notes;
/// First-run setup: permissions and agents.
#[cfg(target_os = "macos")]
pub mod onboarding;
#[cfg(target_os = "macos")]
pub mod panel;
#[cfg(target_os = "macos")]
pub mod permissions;
#[cfg(target_os = "macos")]
pub mod private_api;
#[cfg(target_os = "macos")]
pub mod screen;
pub mod screenshots;
#[cfg(target_os = "macos")]
pub mod selftest;
#[cfg(target_os = "macos")]
pub mod settings;
#[cfg(target_os = "macos")]
pub mod sharing;
#[cfg(target_os = "macos")]
pub mod ui;
/// `notchisland://` links (Shortcuts, scripts).
#[cfg(target_os = "macos")]
pub mod url_scheme;
#[cfg(target_os = "macos")]
pub mod views;
pub mod vision;
#[cfg(target_os = "macos")]
pub mod windows;
