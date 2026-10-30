//! Registry of applications NotchForge knows how to recognise.
//!
//! Notifications arrive from many paths (agent webhooks, the Notification Center
//! database, Accessibility banners). They all carry *some* app identity — a
//! bundle identifier, a free-form source name, or just a title. This module maps
//! those to a [`KnownApp`] so the UI can give each source a stable icon, colour
//! and category, and so per-app filter rules have a canonical key.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Broad category of a notification source. Drives grouping in the UI and some
/// filter defaults (e.g. agents are first-class and default to higher priority).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AppCategory {
    Agent,
    Messaging,
    System,
    Other,
}

/// Applications with first-class support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum KnownApp {
    // Coding agents.
    Claude,
    ChatGpt,
    Cursor,
    Zed,
    OpenCode,
    Qoder,
    GenericAgent,
    // Messaging / social.
    WhatsApp,
    Discord,
    Instagram,
    Facebook,
    Messages,
    Slack,
    Telegram,
    Signal,
}

impl KnownApp {
    pub const ALL: [KnownApp; 15] = [
        KnownApp::Claude,
        KnownApp::ChatGpt,
        KnownApp::Cursor,
        KnownApp::Zed,
        KnownApp::OpenCode,
        KnownApp::Qoder,
        KnownApp::GenericAgent,
        KnownApp::WhatsApp,
        KnownApp::Discord,
        KnownApp::Instagram,
        KnownApp::Facebook,
        KnownApp::Messages,
        KnownApp::Slack,
        KnownApp::Telegram,
        KnownApp::Signal,
    ];

    /// Stable key used in config files and agent payloads (`source = "cursor"`).
    pub fn key(self) -> &'static str {
        match self {
            KnownApp::Claude => "claude",
            KnownApp::ChatGpt => "chatgpt",
            KnownApp::Cursor => "cursor",
            KnownApp::Zed => "zed",
            KnownApp::OpenCode => "opencode",
            KnownApp::Qoder => "qoder",
            KnownApp::GenericAgent => "agent",
            KnownApp::WhatsApp => "whatsapp",
            KnownApp::Discord => "discord",
            KnownApp::Instagram => "instagram",
            KnownApp::Facebook => "facebook",
            KnownApp::Messages => "messages",
            KnownApp::Slack => "slack",
            KnownApp::Telegram => "telegram",
            KnownApp::Signal => "signal",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            KnownApp::Claude => "Claude",
            KnownApp::ChatGpt => "ChatGPT",
            KnownApp::Cursor => "Cursor",
            KnownApp::Zed => "Zed",
            KnownApp::OpenCode => "OpenCode",
            KnownApp::Qoder => "Qoder",
            KnownApp::GenericAgent => "Agent",
            KnownApp::WhatsApp => "WhatsApp",
            KnownApp::Discord => "Discord",
            KnownApp::Instagram => "Instagram",
            KnownApp::Facebook => "Facebook",
            KnownApp::Messages => "Messages",
            KnownApp::Slack => "Slack",
            KnownApp::Telegram => "Telegram",
            KnownApp::Signal => "Signal",
        }
    }

    pub fn category(self) -> AppCategory {
        match self {
            KnownApp::Claude
            | KnownApp::ChatGpt
            | KnownApp::Cursor
            | KnownApp::Zed
            | KnownApp::OpenCode
            | KnownApp::Qoder
            | KnownApp::GenericAgent => AppCategory::Agent,
            _ => AppCategory::Messaging,
        }
    }

    /// Bundle identifiers that belong to this app. Several apps ship more than
    /// one (stable/preview channels, Electron vs. Catalyst rewrites).
    ///
    /// Identifiers for newer tools (OpenCode, Qoder) are best-effort; users can
    /// add more through `[[notifications.aliases]]` in the config.
    pub fn bundle_ids(self) -> &'static [&'static str] {
        match self {
            KnownApp::Claude => &["com.anthropic.claudefordesktop", "com.anthropic.claude"],
            KnownApp::ChatGpt => &["com.openai.chat", "com.openai.codex"],
            KnownApp::Cursor => &["com.todesktop.230313mzl4w4u92"],
            KnownApp::Zed => &["dev.zed.Zed", "dev.zed.Zed-Preview", "dev.zed.Zed-Nightly"],
            KnownApp::OpenCode => &["ai.opencode.desktop", "ai.opencode.app", "dev.opencode.desktop"],
            KnownApp::Qoder => &["com.qoder.ide", "com.qoder.Qoder", "ai.qoder.ide"],
            KnownApp::GenericAgent => &[],
            KnownApp::WhatsApp => &["net.whatsapp.WhatsApp", "desktop.WhatsApp", "WhatsApp"],
            KnownApp::Discord => &["com.hnc.Discord", "com.hnc.DiscordPTB", "com.hnc.DiscordCanary"],
            KnownApp::Instagram => &[],
            KnownApp::Facebook => &["com.facebook.archon", "com.facebook.Messenger"],
            KnownApp::Messages => &["com.apple.MobileSMS", "com.apple.iChat"],
            KnownApp::Slack => &["com.tinyspeck.slackmacgap"],
            KnownApp::Telegram => &["ru.keepcoder.Telegram", "org.telegram.desktop"],
            KnownApp::Signal => &["org.whispersystems.signal-desktop"],
        }
    }

    /// Words that identify a web-delivered notification (browser push) as this
    /// app, e.g. Instagram has no native macOS app so its notifications come
    /// from Safari/Chrome with "Instagram" in the title or subtitle.
    pub fn web_hints(self) -> &'static [&'static str] {
        match self {
            KnownApp::Instagram => &["instagram"],
            KnownApp::Facebook => &["facebook", "messenger"],
            KnownApp::WhatsApp => &["whatsapp"],
            KnownApp::Discord => &["discord"],
            KnownApp::Slack => &["slack"],
            KnownApp::ChatGpt => &["chatgpt"],
            KnownApp::Claude => &["claude.ai"],
            _ => &[],
        }
    }

    /// SF Symbol used when the real application icon is unavailable.
    pub fn symbol(self) -> &'static str {
        match self {
            KnownApp::Claude => "sparkle",
            KnownApp::ChatGpt => "bubble.left.and.text.bubble.right",
            KnownApp::Cursor => "cursorarrow.rays",
            KnownApp::Zed => "chevron.left.forwardslash.chevron.right",
            KnownApp::OpenCode => "terminal",
            KnownApp::Qoder => "hammer",
            KnownApp::GenericAgent => "cpu",
            KnownApp::WhatsApp => "phone.bubble",
            KnownApp::Discord => "gamecontroller",
            KnownApp::Instagram => "camera",
            KnownApp::Facebook => "person.2",
            KnownApp::Messages => "message",
            KnownApp::Slack => "number",
            KnownApp::Telegram => "paperplane",
            KnownApp::Signal => "lock.shield",
        }
    }

    /// Accent colour (sRGB, 0–255) for badges and icon tints.
    pub fn accent(self) -> (u8, u8, u8) {
        match self {
            KnownApp::Claude => (217, 119, 87),
            KnownApp::ChatGpt => (16, 163, 127),
            KnownApp::Cursor => (180, 180, 190),
            KnownApp::Zed => (8, 132, 255),
            KnownApp::OpenCode => (250, 178, 50),
            KnownApp::Qoder => (124, 92, 255),
            KnownApp::GenericAgent => (120, 200, 255),
            KnownApp::WhatsApp => (37, 211, 102),
            KnownApp::Discord => (88, 101, 242),
            KnownApp::Instagram => (225, 48, 108),
            KnownApp::Facebook => (24, 119, 242),
            KnownApp::Messages => (52, 199, 89),
            KnownApp::Slack => (224, 30, 90),
            KnownApp::Telegram => (42, 171, 238),
            KnownApp::Signal => (58, 118, 240),
        }
    }

    /// Parse a config/payload key (`"cursor"`, `"ChatGPT"`, `"claude-code"`).
    pub fn from_key(key: &str) -> Option<KnownApp> {
        let k = key.trim().to_ascii_lowercase().replace(['-', '_', ' '], "");
        Some(match k.as_str() {
            "claude" | "claudecode" | "claudedesktop" | "anthropic" => KnownApp::Claude,
            "chatgpt" | "openai" | "codex" => KnownApp::ChatGpt,
            "cursor" => KnownApp::Cursor,
            "zed" => KnownApp::Zed,
            "opencode" => KnownApp::OpenCode,
            "qoder" => KnownApp::Qoder,
            "agent" | "generic" | "genericagent" => KnownApp::GenericAgent,
            "whatsapp" => KnownApp::WhatsApp,
            "discord" => KnownApp::Discord,
            "instagram" => KnownApp::Instagram,
            "facebook" | "messenger" => KnownApp::Facebook,
            "messages" | "imessage" | "sms" => KnownApp::Messages,
            "slack" => KnownApp::Slack,
            "telegram" => KnownApp::Telegram,
            "signal" => KnownApp::Signal,
            _ => return None,
        })
    }

    pub fn from_bundle_id(bundle_id: &str) -> Option<KnownApp> {
        KnownApp::ALL.into_iter().find(|app| app.bundle_ids().iter().any(|b| b.eq_ignore_ascii_case(bundle_id)))
    }

    /// Recognise browser-delivered notifications by their visible text.
    pub fn from_web_text(text: &str) -> Option<KnownApp> {
        let lower = text.to_ascii_lowercase();
        KnownApp::ALL.into_iter().find(|app| app.web_hints().iter().any(|h| lower.contains(h)))
    }
}

impl fmt::Display for KnownApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

/// Browser bundle identifiers whose notifications should be attributed by
/// content (see [`KnownApp::from_web_text`]).
pub const BROWSER_BUNDLE_IDS: &[&str] = &[
    "com.apple.Safari",
    "com.google.Chrome",
    "com.brave.Browser",
    "company.thebrowser.Browser",
    "org.mozilla.firefox",
    "com.microsoft.edgemac",
];

pub fn is_browser(bundle_id: &str) -> bool {
    BROWSER_BUNDLE_IDS.iter().any(|b| b.eq_ignore_ascii_case(bundle_id))
        || bundle_id.starts_with("com.apple.Safari.WebApp")
}

/// The resolved identity of a notification source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AppRef {
    pub known: Option<KnownApp>,
    pub bundle_id: Option<String>,
    pub display_name: String,
}

impl AppRef {
    pub fn known(app: KnownApp) -> Self {
        Self {
            known: Some(app),
            bundle_id: app.bundle_ids().first().map(|s| (*s).to_owned()),
            display_name: app.display_name().to_owned(),
        }
    }

    /// Resolve from a bundle id plus (optional) visible text used for
    /// browser-delivered notifications.
    pub fn resolve(bundle_id: &str, text_hint: &str, aliases: &[(String, KnownApp)]) -> Self {
        let known = aliases
            .iter()
            .find(|(b, _)| b.eq_ignore_ascii_case(bundle_id))
            .map(|(_, k)| *k)
            .or_else(|| KnownApp::from_bundle_id(bundle_id))
            .or_else(|| if is_browser(bundle_id) { KnownApp::from_web_text(text_hint) } else { None });
        let display_name = match known {
            Some(k) => k.display_name().to_owned(),
            None => humanize_bundle_id(bundle_id),
        };
        Self { known, bundle_id: Some(bundle_id.to_owned()), display_name }
    }

    /// Canonical key for filter rules: the known-app key, else the bundle id,
    /// else the display name.
    pub fn rule_key(&self) -> String {
        match (self.known, &self.bundle_id) {
            (Some(k), _) => k.key().to_owned(),
            (None, Some(b)) => b.clone(),
            (None, None) => self.display_name.clone(),
        }
    }

    pub fn category(&self) -> AppCategory {
        self.known.map_or(AppCategory::Other, KnownApp::category)
    }
}

/// `com.example.MyApp` → `MyApp`.
pub fn humanize_bundle_id(bundle_id: &str) -> String {
    bundle_id.rsplit('.').next().filter(|s| !s.is_empty()).unwrap_or(bundle_id).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        for app in KnownApp::ALL {
            assert_eq!(KnownApp::from_key(app.key()), Some(app), "{app:?}");
        }
    }

    #[test]
    fn bundle_lookup() {
        assert_eq!(KnownApp::from_bundle_id("com.hnc.Discord"), Some(KnownApp::Discord));
        assert_eq!(KnownApp::from_bundle_id("COM.TINYSPECK.SLACKMACGAP"), Some(KnownApp::Slack));
        assert_eq!(KnownApp::from_bundle_id("com.example.unknown"), None);
    }

    #[test]
    fn browser_notifications_are_attributed_by_text() {
        let r = AppRef::resolve("com.google.Chrome", "Instagram · new follower", &[]);
        assert_eq!(r.known, Some(KnownApp::Instagram));
        let r = AppRef::resolve("com.example.app", "Instagram", &[]);
        assert_eq!(r.known, None);
        assert_eq!(r.display_name, "app");
    }

    #[test]
    fn aliases_take_precedence() {
        let aliases = vec![("com.custom.qoder".to_owned(), KnownApp::Qoder)];
        let r = AppRef::resolve("com.custom.qoder", "", &aliases);
        assert_eq!(r.known, Some(KnownApp::Qoder));
        assert_eq!(r.rule_key(), "qoder");
        assert_eq!(r.category(), AppCategory::Agent);
    }
}
