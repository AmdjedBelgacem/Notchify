//! Focus / Do Not Disturb toggling.
//!
//! **Fragile by necessity:** macOS offers no public API to change Focus. The
//! supported, user-visible route is Shortcuts: the user creates two shortcuts
//! ("NotchForge Focus On" / "NotchForge Focus Off", names configurable) using
//! the built-in *Set Focus* action, and NotchForge runs them with the
//! `shortcuts` CLI. Reading the state is done by the Focus provider.

use std::process::{Command, Stdio};

/// Run a shortcut by name without waiting for it.
pub fn run_shortcut(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("no shortcut configured".into());
    }
    Command::new("/usr/bin/shortcuts")
        .arg("run")
        .arg(name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not run shortcut {name:?}: {e}"))
}

/// Toggle: run the "off" shortcut if Focus is known to be on, else "on".
pub fn toggle(active: Option<bool>, on_shortcut: &str, off_shortcut: &str) -> Result<bool, String> {
    let turn_on = active != Some(true);
    run_shortcut(if turn_on { on_shortcut } else { off_shortcut })?;
    Ok(turn_on)
}

/// Whether a shortcut with this exact name exists (`shortcuts list`).
pub fn shortcut_exists(name: &str) -> bool {
    Command::new("/usr/bin/shortcuts")
        .arg("list")
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim() == name))
        .unwrap_or(false)
}
