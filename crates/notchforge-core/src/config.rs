//! TOML configuration.
//!
//! Every field has a default (`#[serde(default)]` on every section) so a user
//! config only needs to contain the keys they want to change, and new versions
//! of NotchForge can add keys without breaking old files.

use crate::apps::KnownApp;
use crate::error::{CoreError, Result};
use crate::model::{Priority, TemperatureUnit};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const APP_DIR_NAME: &str = "NotchForge";
pub const DEFAULT_HTTP_PORT: u16 = 47815;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub appearance: AppearanceConfig,
    pub providers: ProvidersConfig,
    pub weather: WeatherConfig,
    pub agents: AgentsConfig,
    pub notifications: NotificationsConfig,
    pub tray: TrayConfig,
    pub controls: ControlsConfig,
    pub media: MediaConfig,
}

/// Which screen hosts the notch UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenChoice {
    /// The screen with a camera housing; falls back to the main screen.
    #[default]
    Notch,
    /// The screen with the menu bar.
    Main,
    /// The screen under the mouse pointer when displays change.
    Mouse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub launch_at_login: bool,
    pub screen: ScreenChoice,
    /// Pointer must rest in the activation zone this long before hover-expand.
    pub hover_delay_ms: u64,
    /// Grace period after the pointer leaves before collapsing.
    pub collapse_delay_ms: u64,
    /// How long an incoming notification "peeks" out of the collapsed notch.
    pub peek_duration_ms: u64,
    /// Minimum priority that triggers a peek.
    pub peek_min_priority: Priority,
    /// Show the menu bar extra.
    pub show_menu_bar_icon: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            screen: ScreenChoice::Notch,
            hover_delay_ms: 120,
            collapse_delay_ms: 350,
            peek_duration_ms: 4000,
            peek_min_priority: Priority::Normal,
            show_menu_bar_icon: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReducedMotion {
    /// Follow System Settings › Accessibility › Display › Reduce motion.
    #[default]
    System,
    Always,
    Never,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    pub reduced_motion: ReducedMotion,
    /// Multiplier on spring stiffness: 0.5 (lazy) … 2.0 (snappy).
    pub animation_speed: f64,
    pub compact_width: f64,
    pub compact_height: f64,
    pub expanded_width: f64,
    pub expanded_height: f64,
    /// Show live indicators to the left/right of the collapsed notch.
    pub show_wings: bool,
    /// Respect "Increase contrast" by using brighter secondary text.
    pub high_contrast: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            reduced_motion: ReducedMotion::System,
            animation_speed: 1.0,
            compact_width: 520.0,
            compact_height: 144.0,
            expanded_width: 720.0,
            expanded_height: 340.0,
            show_wings: true,
            high_contrast: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProvidersConfig {
    pub battery: bool,
    pub bluetooth: bool,
    pub weather: bool,
    pub agents: bool,
    /// Notification Center database watcher (needs Full Disk Access).
    pub notification_center: bool,
    /// Accessibility banner observer (needs Accessibility).
    pub accessibility_banners: bool,
    pub focus: bool,
    /// Output devices and which apps are playing audio (CoreAudio).
    pub audio: bool,
    /// Spotify & Apple Music now playing, controls and playlists.
    pub media: bool,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            battery: true,
            bluetooth: true,
            weather: true,
            agents: true,
            notification_center: true,
            accessibility_banners: false,
            focus: true,
            audio: true,
            media: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WeatherConfig {
    /// `"auto"` uses CoreLocation; anything else is geocoded as a place name.
    pub location: String,
    /// Explicit coordinates win over `location` when both are set.
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub unit: TemperatureUnit,
    pub refresh_minutes: u64,
    /// Open-Meteo is used by default (no key needed). A custom base URL lets
    /// users point at a self-hosted instance or a commercial endpoint.
    pub api_base: String,
    pub geocoding_base: String,
    /// Optional key appended as `&apikey=` for commercial Open-Meteo.
    pub api_key: Option<String>,
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self {
            location: "auto".to_owned(),
            latitude: None,
            longitude: None,
            unit: TemperatureUnit::Celsius,
            refresh_minutes: 20,
            api_base: "https://api.open-meteo.com/v1/forecast".to_owned(),
            geocoding_base: "https://geocoding-api.open-meteo.com/v1/search".to_owned(),
            api_key: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    pub http_enabled: bool,
    /// Loopback-only HTTP listener (`POST /v1/notify`).
    pub http_port: u16,
    /// Optional shared secret; when set, requests need `Authorization: Bearer <token>`.
    pub token: Option<String>,
    pub socket_enabled: bool,
    /// Overrides the default `<data dir>/agent.sock`.
    pub socket_path: Option<PathBuf>,
    pub inbox_enabled: bool,
    /// Overrides the default `<data dir>/inbox.jsonl`.
    pub inbox_path: Option<PathBuf>,
    /// Ask in the notch before a coding agent's notifications are shown for
    /// the first time. Allow / Deny is remembered per agent.
    pub require_approval: bool,
    /// Announce when a coding-agent CLI session ends (no hooks needed).
    pub watch_sessions: bool,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            http_enabled: true,
            http_port: DEFAULT_HTTP_PORT,
            token: None,
            socket_enabled: true,
            socket_path: None,
            inbox_enabled: true,
            inbox_path: None,
            require_approval: true,
            watch_sessions: true,
        }
    }
}

/// Per-application rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppRule {
    pub enabled: bool,
    /// Overrides the kind-derived priority.
    pub priority: Option<Priority>,
    /// Case-insensitive substrings; matching notifications are dropped.
    pub muted_keywords: Vec<String>,
}

impl Default for AppRule {
    fn default() -> Self {
        Self { enabled: true, priority: None, muted_keywords: Vec::new() }
    }
}

/// Maps an extra bundle id to a known app (e.g. a new Qoder build).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppAlias {
    pub bundle_id: String,
    pub app: KnownApp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationsConfig {
    pub max_items: usize,
    pub dedupe_window_secs: u64,
    pub rate_limit_burst: u32,
    pub rate_limit_refill_secs: u64,
    pub group_window_secs: u64,
    /// While a Focus mode is active only notifications at or above this
    /// priority are shown.
    pub focus_min_priority: Priority,
    /// Show notifications from apps that are not in [`KnownApp`].
    pub allow_other_apps: bool,
    /// Try to close the native banner through Accessibility once the notch
    /// shows the same notification. Fragile; off by default.
    pub suppress_native_banners: bool,
    /// How often the Notification Center DB is checked for changes.
    pub db_poll_interval_ms: u64,
    /// Keyed by [`KnownApp::key`] or by bundle identifier for other apps.
    pub apps: BTreeMap<String, AppRule>,
    pub aliases: Vec<AppAlias>,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        let apps = KnownApp::ALL.iter().map(|a| (a.key().to_owned(), AppRule::default())).collect();
        Self {
            max_items: 100,
            dedupe_window_secs: 10,
            rate_limit_burst: 5,
            rate_limit_refill_secs: 3,
            group_window_secs: 300,
            focus_min_priority: Priority::High,
            allow_other_apps: true,
            suppress_native_banners: false,
            db_poll_interval_ms: 1000,
            apps,
            aliases: Vec::new(),
        }
    }
}

impl NotificationsConfig {
    pub fn rule(&self, key: &str) -> Option<&AppRule> {
        self.apps.get(key)
    }

    pub fn alias_pairs(&self) -> Vec<(String, KnownApp)> {
        self.aliases.iter().map(|a| (a.bundle_id.clone(), a.app)).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrayConfig {
    /// Keep tray contents across restarts.
    pub persist: bool,
    pub max_items: usize,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self { persist: false, max_items: 50 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlsConfig {
    pub brightness: bool,
    pub volume: bool,
    pub resolution: bool,
    pub focus: bool,
    /// Name of the Shortcuts.app shortcut that turns Focus on.
    pub focus_on_shortcut: String,
    /// Name of the Shortcuts.app shortcut that turns Focus off.
    pub focus_off_shortcut: String,
    /// Display mode changes revert automatically unless confirmed in time.
    pub resolution_confirm_secs: u64,
}

/// Default loopback port for the Spotify sign-in redirect.
pub const DEFAULT_SPOTIFY_REDIRECT_PORT: u16 = 47816;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaConfig {
    /// Show the playing track's cover and an equaliser in the collapsed notch.
    pub show_in_wings: bool,
    /// How often players are re-read while one is running (seconds). Changes
    /// announced by the players themselves are picked up immediately.
    pub poll_seconds: u64,
    /// Client ID of *your* Spotify Web API app (developer.spotify.com). Needed
    /// only to list Spotify playlists; playback works without it.
    pub spotify_client_id: String,
    /// Loopback port for the sign-in redirect. Register
    /// `http://127.0.0.1:<port>/callback` as the app's redirect URI.
    pub spotify_redirect_port: u16,
    /// Maximum playlists listed per player.
    pub max_playlists: usize,
    /// Show audio playing in a web browser (YouTube, SoundCloud…) as music.
    pub browser_audio: bool,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            show_in_wings: true,
            poll_seconds: 10,
            spotify_client_id: String::new(),
            spotify_redirect_port: DEFAULT_SPOTIFY_REDIRECT_PORT,
            max_playlists: 100,
            browser_audio: true,
        }
    }
}

impl MediaConfig {
    pub fn spotify_redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.spotify_redirect_port)
    }
}

impl Default for ControlsConfig {
    fn default() -> Self {
        Self {
            brightness: true,
            volume: true,
            resolution: true,
            focus: true,
            focus_on_shortcut: "NotchForge Focus On".to_owned(),
            focus_off_shortcut: "NotchForge Focus Off".to_owned(),
            resolution_confirm_secs: 15,
        }
    }
}

impl Config {
    /// Parse from TOML text and validate.
    pub fn from_toml(text: &str) -> Result<Self> {
        let mut cfg: Config = toml::from_str(text)?;
        cfg.normalize();
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Load from `path`, writing the defaults there if it does not exist.
    pub fn load_or_init(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cfg = Config::default();
                cfg.save(path)?;
                Ok(cfg)
            }
            Err(e) => Err(CoreError::io(path, e)),
        }
    }

    /// Atomically write the config (write to a temp file, then rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
        }
        let text = format!(
            "# Notchisland configuration. See config/notchforge.example.toml for documentation.\n{}",
            self.to_toml()?
        );
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| CoreError::io(&tmp, e))?;
        std::fs::rename(&tmp, path).map_err(|e| CoreError::io(path, e))
    }

    /// Clamp values into sane ranges instead of rejecting the whole file.
    pub fn normalize(&mut self) {
        let a = &mut self.appearance;
        a.animation_speed = a.animation_speed.clamp(0.25, 3.0);
        a.compact_width = a.compact_width.clamp(280.0, 900.0);
        a.compact_height = a.compact_height.clamp(60.0, 300.0);
        a.expanded_width = a.expanded_width.clamp(a.compact_width, 1200.0);
        a.expanded_height = a.expanded_height.clamp(a.compact_height, 700.0);
        let n = &mut self.notifications;
        n.max_items = n.max_items.clamp(10, 1000);
        n.rate_limit_burst = n.rate_limit_burst.max(1);
        n.rate_limit_refill_secs = n.rate_limit_refill_secs.max(1);
        n.db_poll_interval_ms = n.db_poll_interval_ms.clamp(250, 60_000);
        self.weather.refresh_minutes = self.weather.refresh_minutes.clamp(5, 24 * 60);
        self.media.poll_seconds = self.media.poll_seconds.clamp(2, 300);
        self.media.max_playlists = self.media.max_playlists.clamp(1, 500);
        self.media.spotify_client_id = self.media.spotify_client_id.trim().to_owned();
        self.tray.max_items = self.tray.max_items.clamp(1, 500);
        self.controls.resolution_confirm_secs = self.controls.resolution_confirm_secs.clamp(5, 120);
        self.general.hover_delay_ms = self.general.hover_delay_ms.min(2000);
        self.general.collapse_delay_ms = self.general.collapse_delay_ms.min(5000);
        self.general.peek_duration_ms = self.general.peek_duration_ms.clamp(500, 30_000);
    }

    /// Reject configurations that cannot work.
    pub fn validate(&self) -> Result<()> {
        if let (Some(lat), Some(lon)) = (self.weather.latitude, self.weather.longitude) {
            if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
                return Err(CoreError::InvalidConfig(format!("weather coordinates out of range: {lat}, {lon}")));
            }
        }
        if self.weather.latitude.is_some() != self.weather.longitude.is_some() {
            return Err(CoreError::InvalidConfig("weather.latitude and weather.longitude must be set together".into()));
        }
        if self.media.spotify_redirect_port < 1024 {
            return Err(CoreError::InvalidConfig("media.spotify_redirect_port must be ≥ 1024".into()));
        }
        if !self.media.spotify_client_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(CoreError::InvalidConfig("media.spotify_client_id must be alphanumeric".into()));
        }
        if self.agents.http_enabled && self.agents.http_port < 1024 {
            return Err(CoreError::InvalidConfig("agents.http_port must be ≥ 1024".into()));
        }
        if self.weather.location.trim().is_empty() && self.weather.latitude.is_none() {
            return Err(CoreError::InvalidConfig("weather.location must not be empty".into()));
        }
        Ok(())
    }
}

/// Standard filesystem locations.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
}

impl Paths {
    /// `~/Library/Application Support/NotchForge` and `~/Library/Logs/NotchForge`
    /// on macOS; XDG directories elsewhere. `NOTCHFORGE_HOME` overrides both
    /// (useful for tests and running several instances).
    pub fn resolve() -> Result<Self> {
        if let Some(home) = std::env::var_os("NOTCHFORGE_HOME") {
            let base = PathBuf::from(home);
            return Ok(Self::rooted(&base));
        }
        let data_dir = dirs::data_dir().ok_or(CoreError::NoDirectory("application data"))?.join(APP_DIR_NAME);
        let log_dir = if cfg!(target_os = "macos") {
            dirs::home_dir().ok_or(CoreError::NoDirectory("home"))?.join("Library/Logs").join(APP_DIR_NAME)
        } else {
            data_dir.join("logs")
        };
        Ok(Self { config_file: data_dir.join("config.toml"), data_dir, log_dir })
    }

    pub fn rooted(base: &Path) -> Self {
        Self { config_file: base.join("config.toml"), data_dir: base.to_path_buf(), log_dir: base.join("logs") }
    }

    pub fn socket_path(&self, cfg: &AgentsConfig) -> PathBuf {
        cfg.socket_path.clone().unwrap_or_else(|| self.data_dir.join("agent.sock"))
    }

    pub fn inbox_path(&self, cfg: &AgentsConfig) -> PathBuf {
        cfg.inbox_path.clone().unwrap_or_else(|| self.data_dir.join("inbox.jsonl"))
    }

    /// Remembered agent Allow / Deny decisions.
    pub fn trust_file(&self) -> PathBuf {
        self.data_dir.join("trust.json")
    }

    pub fn tray_state(&self) -> PathBuf {
        self.data_dir.join("tray.json")
    }

    /// Cached cover art (safe to delete).
    pub fn artwork_dir(&self) -> PathBuf {
        self.data_dir.join("artwork")
    }

    /// Spotify Web API refresh token (mode 0600).
    pub fn spotify_token(&self) -> PathBuf {
        self.data_dir.join("spotify-token.json")
    }

    pub fn ensure(&self) -> Result<()> {
        for d in [&self.data_dir, &self.log_dir] {
            std::fs::create_dir_all(d).map_err(|e| CoreError::io(d, e))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let cfg = Config::default();
        let text = cfg.to_toml().expect("serialize");
        let back = Config::from_toml(&text).expect("parse");
        assert_eq!(cfg, back);
    }

    #[test]
    fn partial_config_uses_defaults() {
        let cfg = Config::from_toml(
            r#"
            [weather]
            location = "Lisbon"
            unit = "fahrenheit"

            [notifications.apps.discord]
            enabled = false
            "#,
        )
        .expect("parse");
        assert_eq!(cfg.weather.location, "Lisbon");
        assert_eq!(cfg.weather.unit, TemperatureUnit::Fahrenheit);
        assert!(!cfg.notifications.apps["discord"].enabled);
        assert_eq!(cfg.agents.http_port, DEFAULT_HTTP_PORT);
        assert!(cfg.providers.battery);
    }

    #[test]
    fn values_are_clamped() {
        let cfg = Config::from_toml("[appearance]\nanimation_speed = 99.0\nexpanded_width = 10.0").expect("parse");
        assert_eq!(cfg.appearance.animation_speed, 3.0);
        assert!(cfg.appearance.expanded_width >= cfg.appearance.compact_width);
    }

    #[test]
    fn invalid_coordinates_are_rejected() {
        let err = Config::from_toml("[weather]\nlatitude = 120.0\nlongitude = 3.0");
        assert!(matches!(err, Err(CoreError::InvalidConfig(_))));
        let err = Config::from_toml("[weather]\nlatitude = 12.0");
        assert!(matches!(err, Err(CoreError::InvalidConfig(_))));
    }

    #[test]
    fn media_section_defaults_and_validation() {
        let cfg = Config::from_toml("[media]\nspotify_client_id = \" abc123 \"\npoll_seconds = 0").expect("parse");
        assert_eq!(cfg.media.spotify_client_id, "abc123");
        assert_eq!(cfg.media.poll_seconds, 2);
        assert!(cfg.providers.media && cfg.providers.audio);
        assert_eq!(cfg.media.spotify_redirect_uri(), "http://127.0.0.1:47816/callback");
        let err = Config::from_toml("[media]\nspotify_client_id = \"bad id&\"");
        assert!(matches!(err, Err(CoreError::InvalidConfig(_))));
    }

    #[test]
    fn aliases_parse() {
        let cfg = Config::from_toml("[[notifications.aliases]]\nbundle_id = \"com.acme.agent\"\napp = \"qoder\"")
            .expect("parse");
        assert_eq!(cfg.notifications.alias_pairs(), vec![("com.acme.agent".to_owned(), KnownApp::Qoder)]);
    }

    #[test]
    fn shipped_example_config_parses() {
        let cfg = Config::from_toml(include_str!("../../../config/notchforge.example.toml")).expect("example parses");
        assert_eq!(cfg.notifications.apps["discord"].muted_keywords, vec!["@everyone".to_owned()]);
        assert!(!cfg.notifications.apps["com.apple.mail"].enabled);
        assert_eq!(cfg.notifications.apps["claude"].priority, Some(Priority::High));
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("nf-cfg-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        let created = Config::load_or_init(&path).expect("init");
        assert_eq!(created, Config::default());
        let mut changed = created.clone();
        changed.general.hover_delay_ms = 50;
        changed.save(&path).expect("save");
        assert_eq!(Config::load_or_init(&path).expect("load").general.hover_delay_ms, 50);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
