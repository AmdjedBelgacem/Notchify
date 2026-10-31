//! Launch at login.
//!
//! Bundled app (macOS 13+): `SMAppService.mainApp` — the modern, user-visible
//! mechanism (System Settings › General › Login Items).
//! Unbundled binary (development): a per-user LaunchAgent plist.

use objc2_service_management::{SMAppService, SMAppServiceStatus};
use std::path::PathBuf;

const AGENT_LABEL: &str = "dev.notchforge.NotchForge";

fn agent_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/LaunchAgents").join(format!("{AGENT_LABEL}.plist")))
}

pub fn is_enabled() -> bool {
    if crate::permissions::is_bundled() {
        // SAFETY: plain property read on the shared service object.
        return unsafe { SMAppService::mainAppService().status() } == SMAppServiceStatus::Enabled;
    }
    agent_path().is_some_and(|p| p.exists())
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    if crate::permissions::is_bundled() {
        // SAFETY: documented SMAppService calls.
        let service = unsafe { SMAppService::mainAppService() };
        let r = unsafe { if enabled { service.registerAndReturnError() } else { service.unregisterAndReturnError() } };
        return r.map_err(|e| e.localizedDescription().to_string());
    }
    let path = agent_path().ok_or("no home directory")?;
    if !enabled {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        };
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{AGENT_LABEL}</string>
    <key>ProgramArguments</key><array><string>{}</string></array>
    <key>RunAtLoad</key><true/>
    <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#,
        xml_escape(&exe.to_string_lossy())
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, plist).map_err(|e| e.to_string())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
