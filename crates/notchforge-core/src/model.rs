//! Data models shared between providers, the engine and the UI.

use crate::apps::{AppCategory, AppRef};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

/// Monotonic identifier assigned by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NotificationId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

impl Priority {
    pub fn parse(s: &str) -> Option<Priority> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "low" => Priority::Low,
            "normal" | "default" => Priority::Normal,
            "high" => Priority::High,
            "critical" | "urgent" => Priority::Critical,
            _ => return None,
        })
    }
}

/// What happened. Coding agents emit the richer kinds; messaging apps use
/// [`NotificationKind::Message`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    Message,
    TaskFinished,
    NeedsInput,
    Error,
    ToolCall,
    /// An authorization request raised by NotchForge itself (an agent asking
    /// to show notifications, a sign-in waiting on the browser). Only
    /// [`Origin::Internal`] may produce it; no external payload parses to it.
    Authorization,
    #[default]
    Info,
}

impl NotificationKind {
    pub fn parse(s: &str) -> NotificationKind {
        match s.trim().to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "message" | "new_message" => NotificationKind::Message,
            "task_finished"
            | "finished"
            | "done"
            | "stop"
            | "complete"
            | "completed"
            | "agent_turn_complete"
            | "turn_complete" => NotificationKind::TaskFinished,
            "needs_input" | "input" | "permission" | "waiting" | "question" | "notification" => {
                NotificationKind::NeedsInput
            }
            "error" | "failed" | "failure" => NotificationKind::Error,
            "tool_call" | "tool" | "tool_use" => NotificationKind::ToolCall,
            _ => NotificationKind::Info,
        }
    }

    /// Default priority for agent events of this kind.
    pub fn default_priority(self) -> Priority {
        match self {
            NotificationKind::NeedsInput | NotificationKind::Error | NotificationKind::Authorization => Priority::High,
            NotificationKind::TaskFinished | NotificationKind::Message => Priority::Normal,
            NotificationKind::ToolCall | NotificationKind::Info => Priority::Low,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            NotificationKind::Message => "bubble.left.fill",
            NotificationKind::TaskFinished => "checkmark.circle.fill",
            NotificationKind::NeedsInput => "questionmark.bubble.fill",
            NotificationKind::Error => "exclamationmark.triangle.fill",
            NotificationKind::Authorization => "lock.shield.fill",
            NotificationKind::ToolCall => "wrench.and.screwdriver.fill",
            NotificationKind::Info => "info.circle.fill",
        }
    }
}

/// Where a notification was observed. Used for dedupe diagnostics and so the
/// UI can explain e.g. "via Notification Center".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    AgentWebhook,
    AgentSocket,
    AgentInbox,
    NotificationCenterDb,
    AccessibilityBanner,
    Internal,
}

/// Thread-key prefix of an agent-access prompt; the rest is the app's rule key.
pub const AUTH_THREAD_AGENT: &str = "auth:agent:";
/// Thread key of the Spotify sign-in prompt.
pub const AUTH_THREAD_SPOTIFY: &str = "auth:spotify";

/// What an [`NotificationKind::Authorization`] notification asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthTarget {
    /// A coding agent (by rule key) wants its notifications shown: Allow / Deny.
    Agent(String),
    /// Spotify's sign-in is waiting on the browser: Cancel.
    Spotify,
}

/// A notification as produced by a provider, before the pipeline assigns an id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewNotification {
    pub app: AppRef,
    pub origin: Origin,
    pub kind: NotificationKind,
    /// `None` lets the pipeline pick from config / kind defaults.
    pub priority: Option<Priority>,
    pub title: String,
    pub subtitle: Option<String>,
    pub body: String,
    /// Unix epoch milliseconds.
    pub timestamp_ms: i64,
    /// Conversation / task thread used for grouping.
    pub thread: Option<String>,
    /// Deep link or file to open on activation.
    pub url: Option<String>,
}

/// A notification accepted by the pipeline and held in the store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub id: NotificationId,
    pub app: AppRef,
    pub origin: Origin,
    pub kind: NotificationKind,
    pub priority: Priority,
    pub title: String,
    pub subtitle: Option<String>,
    pub body: String,
    pub timestamp_ms: i64,
    pub thread: Option<String>,
    pub url: Option<String>,
    /// Number of notifications folded into this entry (≥ 1).
    pub group_count: u32,
    pub read: bool,
}

impl Notification {
    /// What this authorization prompt is about, if it is one.
    pub fn auth_target(&self) -> Option<AuthTarget> {
        if self.kind != NotificationKind::Authorization {
            return None;
        }
        match self.thread.as_deref()? {
            AUTH_THREAD_SPOTIFY => Some(AuthTarget::Spotify),
            t => t.strip_prefix(AUTH_THREAD_AGENT).map(|k| AuthTarget::Agent(k.to_owned())),
        }
    }

    pub fn category(&self) -> AppCategory {
        self.app.category()
    }

    /// Text suitable for VoiceOver and the clipboard.
    pub fn plain_text(&self) -> String {
        let mut s = format!("{}: {}", self.app.display_name, self.title);
        if let Some(sub) = &self.subtitle {
            s.push_str(" — ");
            s.push_str(sub);
        }
        if !self.body.is_empty() {
            s.push('\n');
            s.push_str(&self.body);
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Power
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BatteryStatus {
    /// 0–100.
    pub percent: u8,
    pub charging: bool,
    pub plugged_in: bool,
    /// Minutes to empty (on battery) or to full (charging), when known.
    pub minutes_remaining: Option<u32>,
    pub present: bool,
}

impl BatteryStatus {
    pub fn symbol(&self) -> &'static str {
        if self.charging {
            return "battery.100percent.bolt";
        }
        match self.percent {
            0..=10 => "battery.0percent",
            11..=37 => "battery.25percent",
            38..=62 => "battery.50percent",
            63..=87 => "battery.75percent",
            _ => "battery.100percent",
        }
    }

    pub fn is_low(&self) -> bool {
        self.present && !self.plugged_in && self.percent <= 20
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    AirPods,
    Headphones,
    Keyboard,
    Mouse,
    Trackpad,
    Watch,
    GameController,
    Other,
}

impl DeviceKind {
    pub fn symbol(self) -> &'static str {
        match self {
            DeviceKind::AirPods => "airpods",
            DeviceKind::Headphones => "headphones",
            DeviceKind::Keyboard => "keyboard",
            DeviceKind::Mouse => "magicmouse",
            DeviceKind::Trackpad => "rectangle.and.hand.point.up.left",
            DeviceKind::Watch => "applewatch",
            DeviceKind::GameController => "gamecontroller",
            DeviceKind::Other => "dot.radiowaves.left.and.right",
        }
    }

    /// Guess from a product name, used when the system does not report a type.
    pub fn guess(name: &str) -> DeviceKind {
        let n = name.to_ascii_lowercase();
        if n.contains("airpods") {
            DeviceKind::AirPods
        } else if n.contains("keyboard") {
            DeviceKind::Keyboard
        } else if n.contains("trackpad") {
            DeviceKind::Trackpad
        } else if n.contains("mouse") {
            DeviceKind::Mouse
        } else if n.contains("watch") {
            DeviceKind::Watch
        } else if n.contains("controller") || n.contains("dualsense") || n.contains("xbox") {
            DeviceKind::GameController
        } else if n.contains("beats") || n.contains("headphone") || n.contains("buds") || n.contains("wh-") {
            DeviceKind::Headphones
        } else {
            DeviceKind::Other
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryComponent {
    Main,
    Left,
    Right,
    Case,
}

impl BatteryComponent {
    pub fn short_label(self) -> &'static str {
        match self {
            BatteryComponent::Main => "",
            BatteryComponent::Left => "L",
            BatteryComponent::Right => "R",
            BatteryComponent::Case => "Case",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceBattery {
    /// Stable identity (Bluetooth address when known, else name).
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub levels: Vec<(BatteryComponent, u8)>,
}

impl DeviceBattery {
    /// Lowest reported level — what matters for "needs charging".
    pub fn min_level(&self) -> Option<u8> {
        self.levels.iter().filter(|(c, _)| *c != BatteryComponent::Case).map(|(_, l)| *l).min()
    }
}

// ---------------------------------------------------------------------------
// Weather
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherCondition {
    Clear,
    PartlyCloudy,
    Cloudy,
    Fog,
    Drizzle,
    Rain,
    FreezingRain,
    Snow,
    Showers,
    Thunderstorm,
    Unknown,
}

impl WeatherCondition {
    pub fn symbol(self, is_day: bool) -> &'static str {
        match (self, is_day) {
            (WeatherCondition::Clear, true) => "sun.max.fill",
            (WeatherCondition::Clear, false) => "moon.stars.fill",
            (WeatherCondition::PartlyCloudy, true) => "cloud.sun.fill",
            (WeatherCondition::PartlyCloudy, false) => "cloud.moon.fill",
            (WeatherCondition::Cloudy, _) => "cloud.fill",
            (WeatherCondition::Fog, _) => "cloud.fog.fill",
            (WeatherCondition::Drizzle, _) => "cloud.drizzle.fill",
            (WeatherCondition::Rain, _) => "cloud.rain.fill",
            (WeatherCondition::FreezingRain, _) => "cloud.sleet.fill",
            (WeatherCondition::Snow, _) => "cloud.snow.fill",
            (WeatherCondition::Showers, _) => "cloud.heavyrain.fill",
            (WeatherCondition::Thunderstorm, _) => "cloud.bolt.rain.fill",
            (WeatherCondition::Unknown, _) => "questionmark.circle",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            WeatherCondition::Clear => "Clear",
            WeatherCondition::PartlyCloudy => "Partly cloudy",
            WeatherCondition::Cloudy => "Cloudy",
            WeatherCondition::Fog => "Fog",
            WeatherCondition::Drizzle => "Drizzle",
            WeatherCondition::Rain => "Rain",
            WeatherCondition::FreezingRain => "Freezing rain",
            WeatherCondition::Snow => "Snow",
            WeatherCondition::Showers => "Showers",
            WeatherCondition::Thunderstorm => "Thunderstorm",
            WeatherCondition::Unknown => "—",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TemperatureUnit {
    #[default]
    Celsius,
    Fahrenheit,
}

impl TemperatureUnit {
    pub fn suffix(self) -> &'static str {
        match self {
            TemperatureUnit::Celsius => "°C",
            TemperatureUnit::Fahrenheit => "°F",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HourlyForecast {
    /// Local hour label, e.g. "14".
    pub hour: String,
    pub temperature: f32,
    pub condition: WeatherCondition,
    pub precipitation_chance: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Weather {
    pub place: String,
    pub temperature: f32,
    pub feels_like: f32,
    pub condition: WeatherCondition,
    pub is_day: bool,
    pub high: f32,
    pub low: f32,
    pub unit: TemperatureUnit,
    pub hourly: Vec<HourlyForecast>,
    pub fetched_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Coordinates {
    pub latitude: f64,
    pub longitude: f64,
}

// ---------------------------------------------------------------------------
// Tray
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TrayItemId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrayItem {
    pub id: TrayItemId,
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub bytes: Option<u64>,
    pub added_at_ms: i64,
}

// ---------------------------------------------------------------------------
// System controls
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayMode {
    /// Opaque, platform-assigned mode identifier.
    pub id: i32,
    /// Logical ("looks like") size in points.
    pub width: u32,
    pub height: u32,
    /// Backing pixel size.
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub refresh_hz: f64,
}

impl DisplayMode {
    pub fn is_hidpi(&self) -> bool {
        self.pixel_width > self.width
    }

    pub fn label(&self) -> String {
        let mut s = format!("{} × {}", self.width, self.height);
        if self.is_hidpi() {
            s.push_str(" (HiDPI)");
        }
        if self.refresh_hz > 0.0 {
            s.push_str(&format!(" @ {:.0} Hz", self.refresh_hz));
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub id: u32,
    pub name: String,
    pub is_builtin: bool,
    pub current_mode_id: Option<i32>,
    pub modes: Vec<DisplayMode>,
}

/// Focus / Do Not Disturb state as far as we can observe it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FocusState {
    /// `None` when the state cannot be read (no Full Disk Access).
    pub active: Option<bool>,
    pub mode_name: Option<String>,
}

// ---------------------------------------------------------------------------
// Audio & media
// ---------------------------------------------------------------------------

/// How an audio device is attached (CoreAudio `kAudioDevicePropertyTransportType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AudioTransport {
    BuiltIn,
    Bluetooth,
    Usb,
    AirPlay,
    Hdmi,
    DisplayPort,
    Thunderbolt,
    Pci,
    Virtual,
    Aggregate,
    #[default]
    Other,
}

impl AudioTransport {
    /// Map a CoreAudio transport four-char code.
    pub fn from_fourcc(code: u32) -> AudioTransport {
        match &code.to_be_bytes() {
            b"bltn" => AudioTransport::BuiltIn,
            b"blue" | b"blea" => AudioTransport::Bluetooth,
            b"usb " => AudioTransport::Usb,
            b"airp" => AudioTransport::AirPlay,
            b"hdmi" => AudioTransport::Hdmi,
            b"dprt" => AudioTransport::DisplayPort,
            b"thun" => AudioTransport::Thunderbolt,
            b"pci " => AudioTransport::Pci,
            b"virt" => AudioTransport::Virtual,
            b"grup" | b"agrp" => AudioTransport::Aggregate,
            _ => AudioTransport::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AudioTransport::BuiltIn => "Built-in",
            AudioTransport::Bluetooth => "Bluetooth",
            AudioTransport::Usb => "USB",
            AudioTransport::AirPlay => "AirPlay",
            AudioTransport::Hdmi => "HDMI",
            AudioTransport::DisplayPort => "DisplayPort",
            AudioTransport::Thunderbolt => "Thunderbolt",
            AudioTransport::Pci => "PCI",
            AudioTransport::Virtual => "Virtual",
            AudioTransport::Aggregate => "Aggregate",
            AudioTransport::Other => "Other",
        }
    }
}

/// An audio output device and what it can do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioDevice {
    /// CoreAudio object id (valid for this boot only).
    pub id: u32,
    /// Persistent device UID; used to switch outputs.
    pub uid: String,
    pub name: String,
    pub transport: AudioTransport,
    /// Current nominal sample rate in Hz.
    pub sample_rate: Option<f64>,
    /// Highest sample rate the device supports, in Hz.
    pub max_sample_rate: Option<f64>,
    /// Bits per channel of the physical stream format.
    pub bit_depth: Option<u32>,
    pub output_channels: u32,
    pub volume: Option<f32>,
    pub muted: Option<bool>,
    /// The current default output device.
    pub is_default: bool,
}

impl AudioDevice {
    pub fn symbol(&self) -> &'static str {
        let n = self.name.to_ascii_lowercase();
        match self.transport {
            AudioTransport::Bluetooth if n.contains("airpods max") => "airpodsmax",
            AudioTransport::Bluetooth if n.contains("airpods") => "airpods",
            AudioTransport::Bluetooth if n.contains("homepod") => "homepod.fill",
            AudioTransport::Bluetooth => "headphones",
            AudioTransport::AirPlay if n.contains("homepod") => "homepod.fill",
            AudioTransport::AirPlay if n.contains("tv") => "appletv.fill",
            AudioTransport::AirPlay => "airplayaudio",
            AudioTransport::Hdmi | AudioTransport::DisplayPort | AudioTransport::Thunderbolt => "tv",
            AudioTransport::Usb => "hifispeaker.fill",
            AudioTransport::BuiltIn if n.contains("headphone") => "headphones",
            AudioTransport::BuiltIn => "laptopcomputer",
            _ => "speaker.wave.2.fill",
        }
    }

    /// Short capability line, e.g. "Bluetooth · 48 kHz · 24-bit · Stereo".
    pub fn capabilities(&self) -> String {
        let mut parts = vec![self.transport.label().to_owned()];
        if let Some(sr) = self.sample_rate {
            parts.push(format_khz(sr));
        }
        if let Some(b) = self.bit_depth.filter(|b| *b > 0) {
            parts.push(format!("{b}-bit"));
        }
        parts.push(match self.output_channels {
            0 => "No outputs".into(),
            1 => "Mono".into(),
            2 => "Stereo".into(),
            n => format!("{n} ch"),
        });
        if let Some(max) = self.max_sample_rate.filter(|m| self.sample_rate.is_none_or(|sr| *m > sr + 1.0)) {
            parts.push(format!("up to {}", format_khz(max)));
        }
        parts.join(" · ")
    }
}

/// `48000.0` → `"48 kHz"`, `44100.0` → `"44.1 kHz"`.
pub fn format_khz(hz: f64) -> String {
    let k = hz / 1000.0;
    if (k - k.round()).abs() < 0.05 { format!("{:.0} kHz", k) } else { format!("{:.1} kHz", k) }
}

/// An application currently producing audio, and where it is going.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioSource {
    /// The app the user would recognise (helpers are mapped to their parent app).
    pub pid: i32,
    pub name: String,
    pub bundle_id: Option<String>,
    /// Path of the `.app` bundle, used for the icon.
    pub app_path: Option<String>,
    /// UIDs of the output devices this app is playing to (empty when unknown).
    pub device_uids: Vec<String>,
}

/// Output devices plus the apps playing audio right now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AudioState {
    pub devices: Vec<AudioDevice>,
    pub sources: Vec<AudioSource>,
    /// Per-app audio activity needs macOS 14.2+; older systems only report devices.
    pub sources_supported: bool,
}

impl AudioState {
    pub fn default_device(&self) -> Option<&AudioDevice> {
        self.devices.iter().find(|d| d.is_default)
    }

    /// Is any app producing audio right now (a browser tab, a video, a call)?
    pub fn is_playing(&self) -> bool {
        !self.sources.is_empty()
    }

    /// The app to feature when nothing from Spotify / Music is playing.
    pub fn playing_app(&self) -> Option<&AudioSource> {
        self.sources.first()
    }

    pub fn device(&self, uid: &str) -> Option<&AudioDevice> {
        self.devices.iter().find(|d| d.uid == uid)
    }

    /// "Spotify → AirPods Pro" style summary of one source.
    pub fn route_label(&self, source: &AudioSource) -> String {
        let names: Vec<&str> =
            source.device_uids.iter().filter_map(|u| self.device(u).map(|d| d.name.as_str())).collect();
        match (names.is_empty(), self.default_device()) {
            (false, _) => names.join(", "),
            (true, Some(d)) => d.name.clone(),
            (true, None) => "Unknown output".into(),
        }
    }
}

/// Media players with a rich integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaPlayer {
    Spotify,
    Music,
    /// Audio playing in a web browser (YouTube, SoundCloud…). Not a scriptable
    /// player: it has no playlists, and transport goes through the media keys.
    /// Deliberately absent from [`MediaPlayer::ALL`].
    Browser,
}

impl MediaPlayer {
    pub const ALL: [MediaPlayer; 2] = [MediaPlayer::Spotify, MediaPlayer::Music];

    pub fn display_name(self) -> &'static str {
        match self {
            MediaPlayer::Spotify => "Spotify",
            MediaPlayer::Music => "Apple Music",
            MediaPlayer::Browser => "Browser",
        }
    }

    pub fn bundle_id(self) -> &'static str {
        match self {
            MediaPlayer::Spotify => "com.spotify.client",
            MediaPlayer::Music => "com.apple.Music",
            MediaPlayer::Browser => "",
        }
    }

    pub fn from_bundle_id(id: &str) -> Option<MediaPlayer> {
        MediaPlayer::ALL.into_iter().find(|p| p.bundle_id().eq_ignore_ascii_case(id))
    }

    /// Brand colour (sRGB).
    pub fn color(self) -> (u8, u8, u8) {
        match self {
            MediaPlayer::Spotify => (30, 215, 96),
            MediaPlayer::Music => (250, 45, 72),
            MediaPlayer::Browser => (10, 132, 255),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackState {
    Playing,
    Paused,
    #[default]
    Stopped,
}

/// What a player is doing right now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NowPlaying {
    pub player: MediaPlayer,
    pub state: PlaybackState,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Track length in seconds.
    pub duration: Option<f64>,
    /// Playback position in seconds at `sampled_at_ms`.
    pub position: Option<f64>,
    pub sampled_at_ms: i64,
    /// Player-specific track identifier (Spotify URI / Music persistent ID).
    pub track_id: Option<String>,
    /// Local file holding the cover art, once downloaded/exported.
    pub artwork_path: Option<PathBuf>,
    pub shuffle: Option<bool>,
    pub repeat: Option<bool>,
    /// Player's own volume, 0–100.
    pub volume: Option<u8>,
    /// For [`MediaPlayer::Browser`]: the browser's bundle id (to bring it forward).
    #[serde(default)]
    pub source_bundle: Option<String>,
}

impl NowPlaying {
    pub fn is_playing(&self) -> bool {
        self.state == PlaybackState::Playing
    }

    /// What to call the source: the player, or the browser's own name.
    pub fn source_name(&self) -> &str {
        match self.player {
            MediaPlayer::Browser if !self.album.is_empty() => &self.album,
            p => p.display_name(),
        }
    }

    /// Whether the source has shuffle, repeat, seeking and playlists.
    pub fn is_rich(&self) -> bool {
        self.player != MediaPlayer::Browser
    }

    /// Position extrapolated to `now_ms` while playing, clamped to the duration.
    pub fn position_at(&self, now_ms: i64) -> Option<f64> {
        let p = self.position?;
        let elapsed = if self.is_playing() { (now_ms - self.sampled_at_ms).max(0) as f64 / 1000.0 } else { 0.0 };
        let p = p + elapsed;
        Some(match self.duration {
            Some(d) if d > 0.0 => p.min(d),
            _ => p,
        })
    }

    /// "Artist — Album", skipping empty parts.
    pub fn subtitle(&self) -> String {
        let album = if self.player == MediaPlayer::Browser { "" } else { self.album.as_str() };
        [self.artist.as_str(), album].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" — ")
    }

    /// Same track (ignoring position/state), so the UI can skip artwork reloads.
    pub fn same_track(&self, other: &NowPlaying) -> bool {
        self.player == other.player
            && match (&self.track_id, &other.track_id) {
                (Some(a), Some(b)) => a == b,
                _ => self.title == other.title && self.artist == other.artist && self.album == other.album,
            }
    }
}

/// A playlist from Spotify or Apple Music.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    pub player: MediaPlayer,
    /// Spotify URI (`spotify:playlist:…`) or Music persistent ID.
    pub id: String,
    pub name: String,
    pub track_count: Option<u32>,
    pub owner: Option<String>,
    /// Remote cover art (Spotify).
    pub artwork_url: Option<String>,
    /// Cover art cached locally by the provider.
    pub artwork_path: Option<PathBuf>,
}

/// Playlist availability per player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
pub enum PlaylistStatus {
    #[default]
    Unknown,
    Loaded,
    /// Spotify needs a sign-in to list playlists.
    NeedsSpotifyLogin,
    /// The browser sign-in is open and waiting for the user to approve.
    SigningIn,
    /// macOS Automation permission was denied for this player.
    NeedsAutomation,
    Failed(String),
}

/// Rich media state: every running player plus playlists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaState {
    /// One entry per *running* player that has a current track.
    pub players: Vec<NowPlaying>,
    /// Which supported players are running at all.
    pub running: Vec<MediaPlayer>,
    pub playlists: Vec<Playlist>,
    pub playlist_status: Vec<(MediaPlayer, PlaylistStatus)>,
    /// Players whose Automation permission was denied.
    pub automation_denied: Vec<MediaPlayer>,
}

impl MediaState {
    /// The player to feature: whichever is playing (the most recently sampled
    /// wins a tie), else the most recently sampled paused one.
    pub fn primary(&self) -> Option<&NowPlaying> {
        let latest = |a: &&NowPlaying, b: &&NowPlaying| a.sampled_at_ms.cmp(&b.sampled_at_ms);
        self.players
            .iter()
            .filter(|p| p.is_playing())
            .max_by(latest)
            .or_else(|| self.players.iter().filter(|p| p.state == PlaybackState::Paused).max_by(latest))
    }

    pub fn player(&self, player: MediaPlayer) -> Option<&NowPlaying> {
        self.players.iter().find(|p| p.player == player)
    }

    pub fn playlists_for(&self, player: MediaPlayer) -> impl Iterator<Item = &Playlist> {
        self.playlists.iter().filter(move |p| p.player == player)
    }

    pub fn playlist_status(&self, player: MediaPlayer) -> PlaylistStatus {
        self.playlist_status.iter().find(|(p, _)| *p == player).map(|(_, s)| s.clone()).unwrap_or_default()
    }

    pub fn is_playing(&self) -> bool {
        self.players.iter().any(NowPlaying::is_playing)
    }
}

/// Transport controls for a player.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action", content = "value")]
pub enum MediaAction {
    PlayPause,
    Next,
    Previous,
    /// Seek to an absolute position in seconds.
    Seek(f64),
    SetShuffle(bool),
    SetRepeat(bool),
    /// Player volume 0–100.
    SetVolume(u8),
    /// Play a playlist by [`Playlist::id`].
    PlayPlaylist(String),
}

// ---------------------------------------------------------------------------
// Permissions & provider status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Accessibility,
    FullDiskAccess,
    Location,
    Notifications,
    Bluetooth,
    /// Apple Events to Spotify / Music (System Settings › Privacy › Automation).
    Automation,
}

impl Permission {
    pub const ALL: [Permission; 6] = [
        Permission::Accessibility,
        Permission::FullDiskAccess,
        Permission::Location,
        Permission::Notifications,
        Permission::Bluetooth,
        Permission::Automation,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Permission::Accessibility => "Accessibility",
            Permission::FullDiskAccess => "Full Disk Access",
            Permission::Location => "Location",
            Permission::Notifications => "Notifications",
            Permission::Bluetooth => "Bluetooth",
            Permission::Automation => "Automation (Spotify & Music)",
        }
    }

    /// One-sentence explanation shown next to the "Request access" button.
    pub fn rationale(self) -> &'static str {
        match self {
            Permission::Accessibility => {
                "Lets Notchisland read notification banners as a fallback and optionally dismiss \
                 the native banner when the notch already shows it."
            }
            Permission::FullDiskAccess => {
                "Lets Notchisland read the Notification Center database so WhatsApp, Discord, \
                 Messages, Slack and others appear in the notch, and read the Focus state."
            }
            Permission::Location => "Used only to fetch local weather. You can type a city in Settings instead.",
            Permission::Notifications => {
                "Lets Notchisland post its own alerts (e.g. low device battery) when the notch \
                 is hidden."
            }
            Permission::Bluetooth => {
                "Not required: accessory batteries are read from the system registry. Listed \
                 for transparency."
            }
            Permission::Automation => {
                "Lets Notchisland ask Spotify and Apple Music what is playing, control playback \
                 and list your playlists. macOS asks once per app."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    Denied,
    NotDetermined,
    NotRequired,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
pub enum ProviderStatus {
    Starting,
    Running,
    Disabled,
    NeedsPermission(Permission),
    Failing(String),
    Stopped,
}

impl ProviderStatus {
    pub fn is_healthy(&self) -> bool {
        matches!(self, ProviderStatus::Running | ProviderStatus::Starting | ProviderStatus::Disabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_parsing_is_lenient() {
        assert_eq!(NotificationKind::parse("Task-Finished"), NotificationKind::TaskFinished);
        assert_eq!(NotificationKind::parse("stop"), NotificationKind::TaskFinished);
        assert_eq!(NotificationKind::parse("agent-turn-complete"), NotificationKind::TaskFinished);
        assert_eq!(NotificationKind::parse("permission"), NotificationKind::NeedsInput);
        assert_eq!(NotificationKind::parse("whatever"), NotificationKind::Info);
    }

    #[test]
    fn priorities_are_ordered() {
        assert!(Priority::Critical > Priority::High);
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
        assert_eq!(Priority::parse("URGENT"), Some(Priority::Critical));
    }

    #[test]
    fn device_min_level_ignores_case() {
        let d = DeviceBattery {
            id: "x".into(),
            name: "AirPods Pro".into(),
            kind: DeviceKind::AirPods,
            levels: vec![(BatteryComponent::Left, 80), (BatteryComponent::Right, 60), (BatteryComponent::Case, 5)],
        };
        assert_eq!(d.min_level(), Some(60));
        assert_eq!(DeviceKind::guess("Magic Trackpad"), DeviceKind::Trackpad);
    }

    fn np(player: MediaPlayer, state: PlaybackState, at: i64) -> NowPlaying {
        NowPlaying {
            player,
            state,
            title: "Teardrop".into(),
            artist: "Massive Attack".into(),
            album: "Mezzanine".into(),
            duration: Some(330.0),
            position: Some(100.0),
            sampled_at_ms: at,
            track_id: Some("t1".into()),
            artwork_path: None,
            shuffle: None,
            repeat: None,
            volume: None,
            source_bundle: None,
        }
    }

    #[test]
    fn primary_player_prefers_playing_then_latest() {
        let mut m = MediaState::default();
        assert!(m.primary().is_none());
        m.players = vec![
            np(MediaPlayer::Music, PlaybackState::Paused, 50),
            np(MediaPlayer::Spotify, PlaybackState::Paused, 10),
        ];
        assert_eq!(m.primary().map(|p| p.player), Some(MediaPlayer::Music));
        m.players.push(np(MediaPlayer::Spotify, PlaybackState::Playing, 5));
        m.players.remove(1);
        assert_eq!(m.primary().map(|p| p.player), Some(MediaPlayer::Spotify));
        assert!(m.is_playing());
    }

    #[test]
    fn position_extrapolates_only_while_playing() {
        let playing = np(MediaPlayer::Spotify, PlaybackState::Playing, 1_000);
        assert_eq!(playing.position_at(6_000), Some(105.0));
        assert_eq!(playing.position_at(10_000_000), Some(330.0), "clamped to duration");
        let paused = np(MediaPlayer::Spotify, PlaybackState::Paused, 1_000);
        assert_eq!(paused.position_at(6_000), Some(100.0));
        assert_eq!(playing.subtitle(), "Massive Attack — Mezzanine");
        assert!(playing.same_track(&paused));
    }

    #[test]
    fn audio_capabilities_and_routes() {
        let dev = AudioDevice {
            id: 7,
            uid: "BT-1".into(),
            name: "AirPods Pro".into(),
            transport: AudioTransport::from_fourcc(u32::from_be_bytes(*b"blue")),
            sample_rate: Some(48_000.0),
            max_sample_rate: Some(48_000.0),
            bit_depth: Some(24),
            output_channels: 2,
            volume: Some(0.5),
            muted: Some(false),
            is_default: true,
        };
        assert_eq!(dev.transport, AudioTransport::Bluetooth);
        assert_eq!(dev.capabilities(), "Bluetooth · 48 kHz · 24-bit · Stereo");
        assert_eq!(dev.symbol(), "airpods");
        assert_eq!(format_khz(44_100.0), "44.1 kHz");
        let state = AudioState {
            devices: vec![dev],
            sources: vec![AudioSource {
                pid: 1,
                name: "Spotify".into(),
                bundle_id: Some("com.spotify.client".into()),
                app_path: None,
                device_uids: vec![],
            }],
            sources_supported: true,
        };
        assert_eq!(state.route_label(&state.sources[0]), "AirPods Pro");
        assert_eq!(MediaPlayer::from_bundle_id("COM.SPOTIFY.CLIENT"), Some(MediaPlayer::Spotify));
    }

    #[test]
    fn display_mode_label() {
        let m =
            DisplayMode { id: 1, width: 1512, height: 982, pixel_width: 3024, pixel_height: 1964, refresh_hz: 120.0 };
        assert_eq!(m.label(), "1512 × 982 (HiDPI) @ 120 Hz");
    }

    #[test]
    fn any_app_producing_audio_counts_as_playing() {
        let mut a = AudioState::default();
        assert!(!a.is_playing() && a.playing_app().is_none());
        a.sources.push(AudioSource {
            pid: 1,
            name: "Google Chrome".into(),
            bundle_id: Some("com.google.Chrome".into()),
            app_path: None,
            device_uids: vec![],
        });
        assert!(a.is_playing());
        assert_eq!(a.playing_app().map(|s| s.name.as_str()), Some("Google Chrome"));
    }
}
