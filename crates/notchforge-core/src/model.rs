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
/// Thread-key prefix of a tool-permission request held for the notch; the
/// rest is the request id the agent's hook is waiting on.
pub const AUTH_THREAD_PERMISSION: &str = "auth:permission:";

/// What an [`NotificationKind::Authorization`] notification asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthTarget {
    /// A coding agent (by rule key) wants its notifications shown: Allow / Deny.
    Agent(String),
    /// Spotify's sign-in is waiting on the browser: Cancel.
    Spotify,
    /// A coding agent waits to run a tool (by request id): Allow / Deny.
    ToolPermission(String),
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
    /// Usage the agent reported along with the event (`notchforge-notify
    /// --input/--output…`). Transport-only: the engine routes it to the usage
    /// tracker and shows no notification.
    #[serde(skip, default)]
    pub tokens: Option<ReportedUsage>,
}

/// Usage an agent reported through the transports instead of writing a log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportedUsage {
    pub counts: TokenCounts,
    pub model: Option<String>,
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
            t => match t.strip_prefix(AUTH_THREAD_PERMISSION) {
                Some(id) => Some(AuthTarget::ToolPermission(id.to_owned())),
                None => t.strip_prefix(AUTH_THREAD_AGENT).map(|k| AuthTarget::Agent(k.to_owned())),
            },
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

/// A coding agent whose token usage can be read or reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum UsageAgent {
    #[default]
    ClaudeCode,
    Codex,
    OpenCode,
    Gemini,
    Qoder,
    Antigravity,
}

impl UsageAgent {
    /// Fixed order used for bar segments and lookups.
    pub const ALL: [UsageAgent; 6] = [
        UsageAgent::ClaudeCode,
        UsageAgent::Codex,
        UsageAgent::OpenCode,
        UsageAgent::Gemini,
        UsageAgent::Qoder,
        UsageAgent::Antigravity,
    ];

    /// Position in [`UsageAgent::ALL`], for the per-agent bar segments.
    pub fn slot(self) -> usize {
        UsageAgent::ALL.iter().position(|a| *a == self).unwrap_or(0)
    }

    /// The agent a webhook/hook `source` name refers to, for reported usage.
    pub fn from_source(source: &str) -> Option<UsageAgent> {
        let s = source.to_ascii_lowercase();
        Some(match s.as_str() {
            "claude" | "claude-code" | "claudecode" => UsageAgent::ClaudeCode,
            "chatgpt" | "codex" | "codex-cli" => UsageAgent::Codex,
            "opencode" => UsageAgent::OpenCode,
            "gemini" | "gemini-cli" => UsageAgent::Gemini,
            "qoder" | "qodercli" => UsageAgent::Qoder,
            "antigravity" => UsageAgent::Antigravity,
            _ => return None,
        })
    }

    pub fn display_name(self) -> &'static str {
        match self {
            UsageAgent::ClaudeCode => "Claude Code",
            UsageAgent::Codex => "Codex CLI",
            UsageAgent::OpenCode => "OpenCode",
            UsageAgent::Gemini => "Gemini",
            UsageAgent::Qoder => "Qoder",
            UsageAgent::Antigravity => "Antigravity",
        }
    }
}

/// Tokens used, by kind. Cache tokens dwarf the rest, so they stay apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TokenCounts {
    /// Input and output: what the page's main figures count.
    pub fn main(&self) -> u64 {
        self.input + self.output
    }

    pub fn cache(&self) -> u64 {
        self.cache_read + self.cache_write
    }

    pub fn add(&mut self, other: &TokenCounts) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

/// `950` → `"950"`, `238_400` → `"238K"`, `1_600_000` → `"1.60M"`, `40_200_000` → `"40.2M"`.
pub fn compact_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_499 => format!("{}K", (n + 500) / 1000),
        999_500..=9_994_999 => format!("{:.2}M", n as f64 / 1e6),
        9_995_000..=999_949_999 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.2}B", n as f64 / 1e9),
    }
}

/// A session among a range's heaviest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSession {
    pub project: String,
    pub agent: UsageAgent,
    pub model: String,
    pub minutes: u32,
    /// Input and output tokens within the range.
    pub tokens: u64,
}

/// One time range of the Agents page. Everything but `counts` excludes cache.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageRange {
    pub label: String,
    pub counts: TokenCounts,
    /// Tokens per hour or day, oldest first; the inner vector is per agent in
    /// [`UsageAgent::ALL`] order.
    pub bars: Vec<Vec<u64>>,
    pub ticks: Vec<String>,
    pub agents: Vec<(UsageAgent, u64)>,
    pub models: Vec<(String, u64)>,
    pub sessions: Vec<UsageSession>,
}

/// A lifecycle event an agent reported through its hooks (Claude Code's
/// `UserPromptSubmit`, `PreToolUse`, `Stop`…): the live sessions follow these
/// the moment they happen instead of waiting for the next log read.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AgentHook {
    pub agent: UsageAgent,
    /// The agent's own session id (Claude Code's `session_id`).
    pub session: String,
    /// The hook's event name, or `PermissionResolved` when a held
    /// permission request ended (answered, timed out or cancelled).
    pub event: String,
    pub tool: Option<String>,
    /// What it is doing or asking, in a few words ("Running cargo test").
    pub detail: Option<String>,
    pub cwd: Option<String>,
    pub at_ms: i64,
    /// For `PermissionRequest` / `PermissionResolved`: the held request.
    pub permission_id: Option<String>,
}

/// A session active in the last half hour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RunningSession {
    pub agent: UsageAgent,
    pub project: String,
    pub tokens: u64,
    /// How full the context window is, where the agent logs its size.
    pub context_percent: Option<u8>,
    /// Logged something in the last few minutes, so it's working now.
    #[serde(default)]
    pub working: bool,
    #[serde(default)]
    pub model: String,
    /// When the session's first line was logged.
    #[serde(default)]
    pub started_ms: i64,
    /// When the current turn began: the person's latest message.
    #[serde(default)]
    pub turn_started_ms: Option<i64>,
    #[serde(default)]
    pub last_ms: i64,
    /// How long its recent turns took, in seconds, oldest first.
    #[serde(default)]
    pub recent_turns: Vec<u32>,
    /// What it did last: "Editing main.rs", "Running cargo test"…
    #[serde(default)]
    pub activity: Option<String>,
}

impl RunningSession {
    /// A typical turn for this session: the median of its recent ones.
    pub fn typical_turn_s(&self) -> Option<u32> {
        if self.recent_turns.len() < 2 {
            return None;
        }
        let mut t = self.recent_turns.clone();
        t.sort_unstable();
        Some(t[t.len() / 2])
    }

    /// Roughly how long the current turn has left, in seconds: a typical turn
    /// less what has run. `Some(0)` when it's already taking longer than usual.
    pub fn eta_s(&self, now_ms: i64) -> Option<u32> {
        let typical = self.typical_turn_s()?;
        let start = self.turn_started_ms.filter(|_| self.working)?;
        let elapsed = ((now_ms - start).max(0) / 1000) as u32;
        Some(typical.saturating_sub(elapsed))
    }
}

/// One usage limit, as a share of its window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitGauge {
    /// "Session", "Week", "Month"…
    pub label: String,
    pub percent: u8,
    pub resets_at_ms: Option<i64>,
}

/// An agent's limits and what its current session is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLimits {
    pub agent: UsageAgent,
    /// Limits are being reported. Codex always logs them; Claude Code needs its
    /// status line connected.
    pub connected: bool,
    pub plan: Option<String>,
    pub running: bool,
    pub project: Option<String>,
    pub gauges: Vec<LimitGauge>,
    pub context_percent: Option<u8>,
}

/// Name a limit by its window: 300 min is the session, 10080 the week.
pub fn limit_label(window_minutes: u64) -> String {
    match window_minutes {
        300 => "Session".into(),
        10_080 => "Week".into(),
        43_200 => "Month".into(),
        m if m >= 1440 => format!("{} days", m / 1440),
        m => format!("{} hours", (m / 60).max(1)),
    }
}

/// Token usage read from the coding agents' own logs (and reported through
/// the agent webhook for tools that keep no logs).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentUsage {
    /// The first read of the logs has finished.
    pub loaded: bool,
    pub claude_found: bool,
    pub codex_found: bool,
    pub opencode_found: bool,
    pub gemini_found: bool,
    pub qoder_found: bool,
    pub antigravity_found: bool,
    /// Today, 7 days, 30 days.
    pub ranges: Vec<UsageRange>,
    pub running: Vec<RunningSession>,
    pub limits: Vec<AgentLimits>,
}

impl AgentUsage {
    /// Whether tokens have been seen for this agent, from any source.
    pub fn found(&self, agent: UsageAgent) -> bool {
        match agent {
            UsageAgent::ClaudeCode => self.claude_found,
            UsageAgent::Codex => self.codex_found,
            UsageAgent::OpenCode => self.opencode_found,
            UsageAgent::Gemini => self.gemini_found,
            UsageAgent::Qoder => self.qoder_found,
            UsageAgent::Antigravity => self.antigravity_found,
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
    /// A shelved clipboard entry (`⌃⌥C`): the copied text or link. File items
    /// leave this `None`; text items carry an empty `path`.
    #[serde(default)]
    pub text: Option<String>,
    /// Pinned text snippets never age out of the tray.
    #[serde(default)]
    pub pinned: bool,
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

/// One upcoming calendar event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CalendarEvent {
    pub title: String,
    /// Unix epoch milliseconds.
    pub starts_at_ms: i64,
    pub ends_at_ms: i64,
    pub all_day: bool,
    pub calendar: String,
    /// Calendar colour as RGB (the colour the user picked in Calendar.app).
    pub color: Option<(u8, u8, u8)>,
    pub location: Option<String>,
    /// Video-call link when the event is an online meeting (Zoom, Meet, …).
    pub meeting_url: Option<String>,
    /// Deep link back to the event in Calendar.app.
    pub url: Option<String>,
}

/// One due reminder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReminderItem {
    pub id: String,
    pub title: String,
    /// Unix epoch milliseconds, when the reminder has a due date.
    pub due_at_ms: Option<i64>,
}

/// A user-started countdown timer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TimerState {
    pub label: String,
    /// Unix epoch ms when the timer finishes.
    pub ends_at_ms: i64,
    /// Full duration, so a progress readout can be shown.
    pub duration_ms: i64,
    /// Fired (waiting to be dismissed).
    pub expired: bool,
    /// A pomodoro cycle: when set, expiring rolls straight into the next
    /// phase (focus → break → focus …) instead of waiting to be dismissed.
    #[serde(default)]
    pub pomodoro: Option<PomodoroCycle>,
    /// Which phase a pomodoro is in right now.
    #[serde(default)]
    pub phase_work: bool,
    /// How many focus rounds of this pomodoro have completed.
    #[serde(default)]
    pub round: u8,
}

/// Focus / break lengths of a pomodoro cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PomodoroCycle {
    pub work_ms: i64,
    pub break_ms: i64,
}

/// Calendar accounts ("connections") and upcoming events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CalendarState {
    /// Connected accounts (e.g. "iCloud", "Google", "Subscribed").
    pub accounts: Vec<String>,
    /// Upcoming events, soonest first.
    pub events: Vec<CalendarEvent>,
    /// Incomplete reminders, soonest due first; undated ones last.
    pub reminders: Vec<ReminderItem>,
    /// Whether Calendar access has been granted.
    pub granted: bool,
    /// Everything on today, morning to night, earliest first.
    #[serde(default)]
    pub today: Vec<CalendarEvent>,
    /// This week, Monday to Sunday: which days have an event.
    #[serde(default)]
    pub busy_days: Vec<bool>,
    /// Which day of this week today is, Monday being 0.
    #[serde(default)]
    pub weekday: u8,
    /// The day of the month each of this week's days falls on.
    #[serde(default)]
    pub week_dates: Vec<u8>,
}

/// A recommended video in YouTube's up-next list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpNextVideo {
    pub title: String,
    pub author: String,
    pub duration: String,
    pub url: String,
    /// Local thumbnail file, once downloaded (the video id's ytimg image).
    #[serde(default)]
    pub thumb: Option<PathBuf>,
}

/// A track waiting in Spotify's play queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct QueueTrack {
    pub title: String,
    pub artist: String,
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
    /// A browser that is making sound but whose tabs cannot be read —
    /// Automation was denied for it. The media page offers to ask.
    #[serde(default)]
    pub browser_denied: Option<String>,
    /// YouTube's up-next recommendations for the browser track, fetched once
    /// per video. Empty unless a YouTube video is playing.
    #[serde(default)]
    pub up_next: Vec<UpNextVideo>,
    /// Spotify's play queue (Web API; needs a sign-in). Empty otherwise.
    #[serde(default)]
    pub spotify_queue: Vec<QueueTrack>,
    /// Lyrics for the current track (LRCLIB), fetched once per track.
    #[serde(default)]
    pub lyrics: Option<Lyrics>,
}

/// A song's lyrics: timestamped lines (milliseconds, for sync) or, when the
/// source had no timings, an untimed block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Lyrics {
    /// `(at_ms, line)` sorted by time; empty for a plain block.
    #[serde(default)]
    pub lines: Vec<(i64, String)>,
    /// True when there were no timestamps (display without highlighting).
    #[serde(default)]
    pub plain: bool,
}

impl Lyrics {
    /// The line playing at `position_ms`: the last one that started at or
    /// before it, ignoring empty (instrumental gap) lines.
    pub fn line_at(&self, position_ms: i64) -> Option<usize> {
        if self.plain {
            return None;
        }
        self.lines
            .iter()
            .rposition(|(at, text)| *at <= position_ms && !line_is_gap(text))
    }

    /// Is any part of the song on screen at `position_ms`?
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// Instrumental-gap lines (`♪` and blanks) never highlight.
fn line_is_gap(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.chars().all(|c| matches!(c, '♪' | '♫' | '·' | '…'))
}

impl MediaState {
    /// The player to feature. A browser that is actually audible wins — its
    /// sound is what the user hears right now (a YouTube video, a call, a
    /// podcast) — then any other playing player (most recently sampled wins a
    /// tie), else the most recently sampled paused one.
    pub fn primary(&self) -> Option<&NowPlaying> {
        let latest = |a: &&NowPlaying, b: &&NowPlaying| a.sampled_at_ms.cmp(&b.sampled_at_ms);
        let playing = self.players.iter().filter(|p| p.is_playing());
        playing
            .clone()
            .find(|p| p.player == MediaPlayer::Browser)
            .or_else(|| playing.max_by(latest))
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
    /// Point the player at this URL. For a browser this navigates the tab
    /// that is already playing, rather than opening a new page.
    Navigate(String),
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
    /// Calendar events via EventKit (System Settings › Privacy › Calendars).
    Calendar,
}

impl Permission {
    pub const ALL: [Permission; 7] = [
        Permission::Accessibility,
        Permission::FullDiskAccess,
        Permission::Location,
        Permission::Notifications,
        Permission::Bluetooth,
        Permission::Automation,
        Permission::Calendar,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Permission::Accessibility => "Accessibility",
            Permission::FullDiskAccess => "Full Disk Access",
            Permission::Location => "Location",
            Permission::Notifications => "Notifications",
            Permission::Bluetooth => "Bluetooth",
            Permission::Automation => "Automation (Spotify & Music)",
            Permission::Calendar => "Calendar",
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
            Permission::Calendar => {
                "Lets Notchisland show your upcoming events from every connected calendar \
                 account (iCloud, Google, Exchange, subscribed). Read-only."
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
        // An audible browser (a YouTube video) outranks another playing player:
        // it is the sound the user hears right now.
        m.players.insert(0, np(MediaPlayer::Browser, PlaybackState::Playing, 4));
        assert_eq!(m.primary().map(|p| p.player), Some(MediaPlayer::Browser));
        // But a paused browser loses to anything playing.
        m.players[0].state = PlaybackState::Paused;
        assert_eq!(m.primary().map(|p| p.player), Some(MediaPlayer::Spotify));
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

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn usage_agents_map_from_source_names() {
        assert_eq!(UsageAgent::from_source("claude"), Some(UsageAgent::ClaudeCode));
        assert_eq!(UsageAgent::from_source("chatgpt"), Some(UsageAgent::Codex));
        assert_eq!(UsageAgent::from_source("opencode"), Some(UsageAgent::OpenCode));
        assert_eq!(UsageAgent::from_source("Gemini"), Some(UsageAgent::Gemini));
        assert_eq!(UsageAgent::from_source("qodercli"), Some(UsageAgent::Qoder));
        assert_eq!(UsageAgent::from_source("antigravity"), Some(UsageAgent::Antigravity));
        assert_eq!(UsageAgent::from_source("cursor"), None);
    }

    #[test]
    fn token_counts_read_short() {
        assert_eq!(compact_tokens(950), "950");
        assert_eq!(compact_tokens(238_400), "238K");
        assert_eq!(compact_tokens(999_499), "999K");
        assert_eq!(compact_tokens(999_500), "1.00M");
        assert_eq!(compact_tokens(1_600_000), "1.60M");
        assert_eq!(compact_tokens(40_200_000), "40.2M");
        assert_eq!(compact_tokens(2_500_000_000), "2.50B");
    }

    #[test]
    fn limits_are_named_by_their_window() {
        assert_eq!(limit_label(300), "Session");
        assert_eq!(limit_label(10_080), "Week");
        assert_eq!(limit_label(43_200), "Month");
        assert_eq!(limit_label(2880), "2 days");
        assert_eq!(limit_label(120), "2 hours");
    }
}
