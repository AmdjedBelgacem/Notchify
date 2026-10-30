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
            NotificationKind::NeedsInput | NotificationKind::Error => Priority::High,
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
            DeviceKind::AirPods => "airpodspro",
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
}

impl Permission {
    pub const ALL: [Permission; 5] = [
        Permission::Accessibility,
        Permission::FullDiskAccess,
        Permission::Location,
        Permission::Notifications,
        Permission::Bluetooth,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Permission::Accessibility => "Accessibility",
            Permission::FullDiskAccess => "Full Disk Access",
            Permission::Location => "Location",
            Permission::Notifications => "Notifications",
            Permission::Bluetooth => "Bluetooth",
        }
    }

    /// One-sentence explanation shown next to the "Request access" button.
    pub fn rationale(self) -> &'static str {
        match self {
            Permission::Accessibility => {
                "Lets NotchForge read notification banners as a fallback and optionally dismiss \
                 the native banner when the notch already shows it."
            }
            Permission::FullDiskAccess => {
                "Lets NotchForge read the Notification Center database so WhatsApp, Discord, \
                 Messages, Slack and others appear in the notch, and read the Focus state."
            }
            Permission::Location => "Used only to fetch local weather. You can type a city in Settings instead.",
            Permission::Notifications => {
                "Lets NotchForge post its own alerts (e.g. low device battery) when the notch \
                 is hidden."
            }
            Permission::Bluetooth => {
                "Not required: accessory batteries are read from the system registry. Listed \
                 for transparency."
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

    #[test]
    fn display_mode_label() {
        let m =
            DisplayMode { id: 1, width: 1512, height: 982, pixel_width: 3024, pixel_height: 1964, refresh_hz: 120.0 };
        assert_eq!(m.label(), "1512 × 982 (HiDPI) @ 120 Hz");
    }
}
