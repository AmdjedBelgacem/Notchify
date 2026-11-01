//! The engine: single owner of application state.
//!
//! Providers send [`ProviderEvent`]s, the UI sends [`Command`]s; the engine
//! applies both to [`AppState`] and publishes immutable [`Snapshot`]s to a
//! [`UiSink`]. Snapshot publication is coalesced (at most one per
//! [`COALESCE`]) so a burst of a hundred notifications produces a handful of
//! UI updates, never a hundred.

use crate::apps::AppRef;
use crate::config::{Config, Paths};
use crate::model::{
    AUTH_THREAD_AGENT, AUTH_THREAD_SPOTIFY, AudioState, BatteryStatus, DeviceBattery, FocusState, MediaPlayer,
    MediaState, NewNotification, Notification, NotificationId, NotificationKind, Origin, PlaylistStatus, Priority,
    ProviderStatus, TrayItem, TrayItemId, Weather,
};
use crate::pipeline::{Outcome, Pipeline, PipelineFlags};
use crate::provider::{ProviderCommand, ProviderEvent, Shutdown};
use crate::time::now_ms;
use crate::tray::Tray;
use crate::trust::{Decision, TrustStore};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, broadcast, mpsc, watch};
use tokio::time::Instant;

/// Minimum interval between two published snapshots.
pub const COALESCE: Duration = Duration::from_millis(16);
/// Minimum interval between two peeks.
pub const PEEK_COOLDOWN_MS: i64 = 2000;
/// Device battery level that triggers a one-off internal alert.
pub const LOW_DEVICE_BATTERY: u8 = 15;

/// Requests from the UI.
#[derive(Debug, Clone)]
pub enum Command {
    DismissNotification(NotificationId),
    /// Dismiss every notification from an app (by rule key).
    DismissApp(String),
    ClearNotifications,
    MarkAllRead,
    TrayAdd(Vec<PathBuf>),
    TrayRemove(TrayItemId),
    TrayClear,
    SetPaused(bool),
    /// Persist and broadcast a new config.
    UpdateConfig(Box<Config>),
    /// Ask providers to refresh now (panel opened).
    Refresh,
    /// Inject a notification (used by the UI for internal alerts and tests).
    Inject(Box<NewNotification>),
    /// Forward a request to the providers (media controls, output switching…).
    Provider(ProviderCommand),
    /// Answer a coding agent's request to show notifications (by rule key).
    Authorize {
        key: String,
        allow: bool,
    },
    /// Forget every remembered Allow / Deny, so agents are asked again.
    ForgetAgentDecisions,
}

/// Immutable view of the state handed to the UI.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub generation: u64,
    pub now_ms: i64,
    pub battery: Option<BatteryStatus>,
    pub devices: Vec<DeviceBattery>,
    pub weather: Option<Weather>,
    pub focus: FocusState,
    pub audio: AudioState,
    pub media: MediaState,
    /// Ranked: priority, then recency.
    pub notifications: Vec<Notification>,
    pub latest: Option<Notification>,
    pub unread: usize,
    pub tray: Vec<TrayItem>,
    pub providers: BTreeMap<&'static str, ProviderStatus>,
    pub paused: bool,
}

impl Snapshot {
    /// Whether the collapsed notch should show live indicators either side.
    pub fn wants_wings(&self) -> bool {
        self.wants_wings_with(true)
    }

    /// Like [`Snapshot::wants_wings`], optionally ignoring playing media.
    pub fn wants_wings_with(&self, media: bool) -> bool {
        self.unread > 0
            || !self.tray.is_empty()
            || (media && (self.media.is_playing() || self.audio.is_playing()))
            || self.battery.as_ref().is_some_and(|b| b.charging || b.is_low())
    }
}

/// Where snapshots go. Implemented by the macOS bridge (dispatching onto the
/// main thread) and by the headless logger.
pub trait UiSink: Send + Sync + 'static {
    fn publish(&self, snapshot: Arc<Snapshot>);
    /// A notification important enough to peek out of the collapsed notch.
    fn peek(&self, notification: Notification);
    /// The config changed (UI may need to re-layout).
    fn config_changed(&self, _config: Arc<Config>) {}
    /// A notification observed from the system (DB/AX) is now displayed by the
    /// notch; the UI may try to dismiss the native banner.
    fn native_banner_handled(&self, _notification: &Notification) {}
}

#[derive(Default)]
struct AppState {
    battery: Option<BatteryStatus>,
    devices: Vec<DeviceBattery>,
    weather: Option<Weather>,
    focus: FocusState,
    audio: AudioState,
    media: MediaState,
    pipeline: Pipeline,
    tray: Tray,
    providers: BTreeMap<&'static str, ProviderStatus>,
    paused: bool,
    low_device_alerted: HashSet<String>,
    low_mac_alerted: Option<u8>,
    /// Remembered Allow / Deny per coding agent.
    trust: TrustStore,
    /// Notifications from agents still waiting for a decision, by rule key.
    held: BTreeMap<String, Vec<NewNotification>>,
}

/// Cheap handle for sending commands from any thread.
#[derive(Clone)]
pub struct EngineHandle {
    commands: mpsc::UnboundedSender<Command>,
}

impl EngineHandle {
    /// Fire-and-forget; silently ignored after shutdown.
    pub fn send(&self, cmd: Command) {
        let _ = self.commands.send(cmd);
    }
}

pub struct Engine {
    state: AppState,
    config_tx: watch::Sender<Arc<Config>>,
    paths: Arc<Paths>,
    sink: Arc<dyn UiSink>,
    refresh: Arc<Notify>,
    provider_commands: broadcast::Sender<ProviderCommand>,
    events: mpsc::Receiver<ProviderEvent>,
    commands: mpsc::UnboundedReceiver<Command>,
    generation: u64,
    last_peek_ms: i64,
}

impl Engine {
    pub fn new(
        config_tx: watch::Sender<Arc<Config>>,
        paths: Arc<Paths>,
        sink: Arc<dyn UiSink>,
        refresh: Arc<Notify>,
        provider_commands: broadcast::Sender<ProviderCommand>,
        events: mpsc::Receiver<ProviderEvent>,
    ) -> (Self, EngineHandle) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let mut state = AppState::default();
        if config_tx.borrow().tray.persist {
            match Tray::load(&paths.tray_state()) {
                Ok(t) => state.tray = t,
                Err(e) => tracing::warn!(error = %e, "could not restore tray"),
            }
        }
        match TrustStore::load(&paths.trust_file()) {
            Ok(t) => state.trust = t,
            Err(e) => tracing::warn!(error = %e, "could not read remembered agent decisions"),
        }
        let engine = Self {
            state,
            config_tx,
            paths,
            sink,
            refresh,
            provider_commands,
            events,
            commands: cmd_rx,
            generation: 0,
            last_peek_ms: i64::MIN / 2,
        };
        (engine, EngineHandle { commands: cmd_tx })
    }

    fn config(&self) -> Arc<Config> {
        self.config_tx.borrow().clone()
    }

    /// Run until shutdown. Publishes an initial snapshot immediately.
    pub async fn run(mut self, mut shutdown: Shutdown) {
        self.publish();
        let mut last_publish = Instant::now();
        let mut dirty = false;
        loop {
            let deadline = last_publish + COALESCE;
            tokio::select! {
                biased;
                _ = shutdown.wait() => break,
                Some(cmd) = self.commands.recv() => {
                    dirty |= self.apply_command(cmd);
                }
                Some(ev) = self.events.recv() => {
                    dirty |= self.apply_event(ev);
                }
                _ = tokio::time::sleep_until(deadline), if dirty => {}
                else => break,
            }
            if dirty && Instant::now() >= last_publish + COALESCE {
                self.publish();
                last_publish = Instant::now();
                dirty = false;
            }
        }
        self.persist_tray();
        tracing::debug!("engine stopped");
    }

    fn persist_tray(&self) {
        let cfg = self.config();
        let path = self.paths.tray_state();
        if cfg.tray.persist {
            if let Err(e) = self.state.tray.save(&path) {
                tracing::warn!(error = %e, "could not persist tray");
            }
        } else if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }

    pub fn snapshot(&mut self) -> Snapshot {
        self.generation += 1;
        let s = &self.state;
        Snapshot {
            generation: self.generation,
            now_ms: now_ms(),
            battery: s.battery.clone(),
            devices: s.devices.clone(),
            weather: s.weather.clone(),
            focus: s.focus.clone(),
            audio: s.audio.clone(),
            media: s.media.clone(),
            notifications: s.pipeline.ranked(),
            latest: s.pipeline.latest().cloned(),
            unread: s.pipeline.unread_count(),
            tray: s.tray.items().to_vec(),
            providers: s.providers.clone(),
            paused: s.paused,
        }
    }

    fn publish(&mut self) {
        let snap = Arc::new(self.snapshot());
        self.sink.publish(snap);
    }

    /// Returns whether state changed.
    pub fn apply_event(&mut self, ev: ProviderEvent) -> bool {
        match ev {
            ProviderEvent::Notification(n) => self.ingest(n),
            ProviderEvent::Battery(b) => {
                self.check_mac_battery(&b);
                let changed = self.state.battery.as_ref() != Some(&b);
                self.state.battery = Some(b);
                changed
            }
            ProviderEvent::Devices(d) => {
                self.check_device_batteries(&d);
                let changed = self.state.devices != d;
                self.state.devices = d;
                changed
            }
            ProviderEvent::Weather(w) => {
                self.state.weather = Some(w);
                true
            }
            ProviderEvent::Focus(f) => {
                let changed = self.state.focus != f;
                self.state.focus = f;
                changed
            }
            ProviderEvent::Audio(a) => {
                let changed = self.state.audio != a;
                self.state.audio = a;
                changed
            }
            ProviderEvent::Media(m) => {
                let changed = self.state.media != m;
                let was_signing_in = self.spotify_signing_in();
                self.state.media = m;
                if changed {
                    self.spotify_prompt(was_signing_in);
                }
                changed
            }
            ProviderEvent::Status { provider, status } => {
                self.state.providers.insert(provider, status.clone()) != Some(status)
            }
        }
    }

    pub fn apply_command(&mut self, cmd: Command) -> bool {
        let cfg = self.config();
        match cmd {
            Command::DismissNotification(id) => self.state.pipeline.dismiss(id),
            Command::DismissApp(key) => {
                self.state.pipeline.dismiss_app(&key);
                true
            }
            Command::ClearNotifications => {
                self.state.pipeline.clear();
                true
            }
            Command::MarkAllRead => self.state.pipeline.mark_all_read(),
            Command::TrayAdd(paths) => {
                let added = self.state.tray.add(paths, cfg.tray.max_items, now_ms());
                self.persist_tray();
                added > 0 || !self.state.tray.items().is_empty()
            }
            Command::TrayRemove(id) => {
                let removed = self.state.tray.remove(id).is_some();
                self.persist_tray();
                removed
            }
            Command::TrayClear => {
                self.state.tray.clear();
                self.persist_tray();
                true
            }
            Command::SetPaused(p) => {
                let changed = self.state.paused != p;
                self.state.paused = p;
                changed
            }
            Command::UpdateConfig(new) => {
                let mut new = *new;
                new.normalize();
                if let Err(e) = new.validate() {
                    tracing::warn!(error = %e, "rejected config update");
                    return false;
                }
                if let Err(e) = new.save(&self.paths.config_file) {
                    tracing::warn!(error = %e, "could not save config");
                }
                let new = Arc::new(new);
                self.config_tx.send_replace(new.clone());
                self.sink.config_changed(new);
                self.persist_tray();
                true
            }
            Command::Refresh => {
                self.refresh.notify_waiters();
                let pruned = self.state.tray.prune_missing();
                if pruned {
                    self.persist_tray();
                }
                pruned
            }
            Command::Inject(n) => self.ingest(*n),
            Command::Provider(c) => {
                // No receivers just means the relevant provider is disabled.
                let _ = self.provider_commands.send(c);
                false
            }
            Command::Authorize { key, allow } => self.authorize(&key, allow),
            Command::ForgetAgentDecisions => {
                self.state.trust.clear();
                self.persist_trust();
                true
            }
        }
    }

    // ── authorization ───────────────────────────────────────────────────────

    fn persist_trust(&self) {
        if let Err(e) = self.state.trust.save(&self.paths.trust_file()) {
            tracing::warn!(error = %e, "could not save agent decisions");
        }
    }

    fn prompt_id(&self, thread: &str) -> Option<NotificationId> {
        self.state.pipeline.ranked().into_iter().find(|n| n.thread.as_deref() == Some(thread)).map(|n| n.id)
    }

    /// An agent's first notification: hold it and ask in the notch.
    fn hold_for_consent(&mut self, key: String, n: NewNotification) -> bool {
        const MAX_HELD: usize = 50;
        let app = n.app.clone();
        let held = self.state.held.entry(key.clone()).or_default();
        if held.len() >= MAX_HELD {
            held.remove(0);
        }
        held.push(n);
        let thread = format!("{AUTH_THREAD_AGENT}{key}");
        if self.prompt_id(&thread).is_some() {
            return true;
        }
        tracing::info!(%key, "agent notification held for approval");
        let prompt = NewNotification {
            title: format!("Allow {} in the notch?", app.display_name),
            body: "It asked to show notifications. Nothing appears until you decide.".into(),
            app,
            origin: Origin::Internal,
            kind: NotificationKind::Authorization,
            priority: Some(Priority::High),
            subtitle: None,
            timestamp_ms: now_ms(),
            thread: Some(thread),
            url: None,
        };
        self.ingest_now(prompt);
        true
    }

    fn authorize(&mut self, key: &str, allow: bool) -> bool {
        if let Some(id) = self.prompt_id(&format!("{AUTH_THREAD_AGENT}{key}")) {
            self.state.pipeline.dismiss(id);
        }
        self.state.trust.set(key, if allow { Decision::Allow } else { Decision::Deny });
        self.persist_trust();
        let held = self.state.held.remove(key).unwrap_or_default();
        tracing::info!(%key, allow, released = held.len(), "agent decision");
        if allow {
            for n in held {
                self.ingest_now(n);
            }
        }
        true
    }

    fn spotify_signing_in(&self) -> bool {
        self.state.media.playlist_status(MediaPlayer::Spotify) == PlaylistStatus::SigningIn
    }

    /// Mirror Spotify's browser sign-in as a prompt in the notch, and report
    /// how it ended.
    fn spotify_prompt(&mut self, was_signing_in: bool) {
        let now_signing_in = self.spotify_signing_in();
        let spotify =
            AppRef { known: None, bundle_id: Some("com.spotify.client".into()), display_name: "Spotify".into() };
        let note = |kind, priority, title: &str, body: String, thread: Option<String>| NewNotification {
            app: spotify.clone(),
            origin: Origin::Internal,
            kind,
            priority: Some(priority),
            title: title.into(),
            subtitle: None,
            body,
            timestamp_ms: now_ms(),
            thread,
            url: None,
        };
        if now_signing_in && !was_signing_in {
            self.ingest_now(note(
                NotificationKind::Authorization,
                Priority::High,
                "Approve Notchisland in your browser",
                "Spotify's sign-in is open. Allow access there, then come back.".into(),
                Some(AUTH_THREAD_SPOTIFY.into()),
            ));
        } else if was_signing_in && !now_signing_in {
            if let Some(id) = self.prompt_id(AUTH_THREAD_SPOTIFY) {
                self.state.pipeline.dismiss(id);
            }
            match self.state.media.playlist_status(MediaPlayer::Spotify) {
                PlaylistStatus::Failed(why) => {
                    self.ingest_now(note(NotificationKind::Error, Priority::High, "Spotify sign-in failed", why, None))
                }
                PlaylistStatus::NeedsSpotifyLogin => false, // cancelled
                _ => self.ingest_now(note(
                    NotificationKind::Info,
                    Priority::Normal,
                    "Spotify connected",
                    "Your playlists are loading in Now Playing.".into(),
                    None,
                )),
            };
        }
    }

    /// Entry point for every incoming notification: agents may need consent
    /// first, and nothing external may pose as an authorization prompt.
    fn ingest(&mut self, n: NewNotification) -> bool {
        if n.kind == NotificationKind::Authorization && n.origin != Origin::Internal {
            tracing::warn!(origin = ?n.origin, "dropped a forged authorization prompt");
            return false;
        }
        let from_agent = matches!(n.origin, Origin::AgentWebhook | Origin::AgentSocket | Origin::AgentInbox);
        if from_agent && self.config().agents.require_approval {
            let key = n.app.rule_key();
            match self.state.trust.decision(&key) {
                Some(Decision::Allow) => {}
                Some(Decision::Deny) => return false,
                None => return self.hold_for_consent(key, n),
            }
        }
        self.ingest_now(n)
    }

    fn ingest_now(&mut self, n: NewNotification) -> bool {
        let cfg = self.config();
        let flags = PipelineFlags { paused: self.state.paused, focus_active: self.state.focus.active == Some(true) };
        let from_system = matches!(n.origin, Origin::NotificationCenterDb | Origin::AccessibilityBanner);
        let now = now_ms();
        let (origin, app) = (n.origin, n.app.display_name.clone());
        let outcome = self.state.pipeline.ingest(n, &cfg.notifications, flags, now);
        tracing::debug!(?outcome, ?origin, %app, "notification ingested");
        let (id, peekable) = match outcome {
            Outcome::Accepted { id, priority } => (id, priority >= cfg.general.peek_min_priority),
            Outcome::Grouped { id, rate_limited } => (id, !rate_limited),
            Outcome::Duplicate | Outcome::Filtered(_) => return false,
        };
        if let Some(item) = self.state.pipeline.get(id).cloned() {
            if from_system && cfg.notifications.suppress_native_banners {
                self.sink.native_banner_handled(&item);
            }
            if peekable && item.priority >= cfg.general.peek_min_priority && now - self.last_peek_ms >= PEEK_COOLDOWN_MS
            {
                self.last_peek_ms = now;
                self.sink.peek(item);
            }
        }
        true
    }

    fn internal(&mut self, title: String, body: String, priority: Priority) {
        let n = NewNotification {
            app: AppRef { known: None, bundle_id: None, display_name: "Notchisland".into() },
            origin: Origin::Internal,
            kind: NotificationKind::Info,
            priority: Some(priority),
            title,
            subtitle: None,
            body,
            timestamp_ms: now_ms(),
            thread: None,
            url: None,
        };
        self.ingest(n);
    }

    fn check_device_batteries(&mut self, devices: &[DeviceBattery]) {
        for d in devices {
            match d.min_level() {
                Some(level) if level <= LOW_DEVICE_BATTERY => {
                    if self.state.low_device_alerted.insert(d.id.clone()) {
                        self.internal(format!("{} battery low", d.name), format!("{level}% remaining"), Priority::High);
                    }
                }
                Some(level) if level > LOW_DEVICE_BATTERY + 5 => {
                    self.state.low_device_alerted.remove(&d.id);
                }
                _ => {}
            }
        }
    }

    fn check_mac_battery(&mut self, b: &BatteryStatus) {
        if !b.present || b.plugged_in {
            self.state.low_mac_alerted = None;
            return;
        }
        for threshold in [10u8, 20] {
            if b.percent <= threshold && self.state.low_mac_alerted.is_none_or(|t| t > threshold) {
                self.state.low_mac_alerted = Some(threshold);
                self.internal("Battery low".into(), format!("{}% remaining", b.percent), Priority::High);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::KnownApp;
    use crate::model::{
        BatteryComponent, DeviceKind, MediaAction, MediaPlayer, MediaState, NowPlaying, PlaybackState, PlaylistStatus,
    };
    use crate::provider::shutdown_pair;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        snaps: Mutex<Vec<Arc<Snapshot>>>,
        peeks: Mutex<Vec<Notification>>,
    }

    impl UiSink for Recorder {
        fn publish(&self, s: Arc<Snapshot>) {
            self.snaps.lock().expect("lock").push(s);
        }
        fn peek(&self, n: Notification) {
            self.peeks.lock().expect("lock").push(n);
        }
    }

    /// An engine that shows agents' notifications straight away.
    fn engine() -> (Engine, EngineHandle, Arc<Recorder>, mpsc::Sender<ProviderEvent>) {
        let mut cfg = Config::default();
        cfg.agents.require_approval = false;
        engine_with(cfg)
    }

    fn engine_with(cfg: Config) -> (Engine, EngineHandle, Arc<Recorder>, mpsc::Sender<ProviderEvent>) {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("nf-engine-{}-{}-{n}", std::process::id(), now_ms()));
        let paths = Arc::new(Paths::rooted(&dir));
        let (cfg_tx, _rx) = watch::channel(Arc::new(cfg));
        let rec = Arc::new(Recorder::default());
        let (ev_tx, ev_rx) = mpsc::channel(1024);
        let (e, h) =
            Engine::new(cfg_tx, paths, rec.clone(), Arc::new(Notify::new()), crate::provider::command_channel(), ev_rx);
        (e, h, rec, ev_tx)
    }

    fn agent_note(title: &str) -> NewNotification {
        NewNotification {
            app: AppRef::known(KnownApp::Claude),
            origin: Origin::AgentWebhook,
            kind: NotificationKind::TaskFinished,
            priority: None,
            title: title.into(),
            subtitle: None,
            body: String::new(),
            timestamp_ms: now_ms(),
            thread: None,
            url: None,
        }
    }

    #[test]
    fn notifications_peek_with_cooldown() {
        let (mut e, _h, rec, _tx) = engine();
        assert!(e.apply_event(ProviderEvent::Notification(agent_note("one"))));
        assert!(e.apply_event(ProviderEvent::Notification(agent_note("two"))));
        assert_eq!(rec.peeks.lock().expect("lock").len(), 1);
        let snap = e.snapshot();
        assert_eq!(snap.unread, 2);
        assert!(snap.wants_wings());
    }

    #[test]
    fn low_device_battery_alerts_once() {
        let (mut e, _h, _rec, _tx) = engine();
        let dev = |lvl| DeviceBattery {
            id: "aa:bb".into(),
            name: "AirPods".into(),
            kind: DeviceKind::AirPods,
            levels: vec![(BatteryComponent::Left, lvl)],
        };
        e.apply_event(ProviderEvent::Devices(vec![dev(10)]));
        e.apply_event(ProviderEvent::Devices(vec![dev(9)]));
        assert_eq!(e.snapshot().notifications.len(), 1);
        // Recharged, then low again → alert again (grouped into the same entry).
        e.apply_event(ProviderEvent::Devices(vec![dev(80)]));
        e.apply_event(ProviderEvent::Devices(vec![dev(12)]));
        assert_eq!(e.snapshot().notifications[0].group_count, 2);
    }

    #[test]
    fn mac_battery_thresholds() {
        let (mut e, _h, _rec, _tx) = engine();
        let b = |p| BatteryStatus {
            percent: p,
            charging: false,
            plugged_in: false,
            minutes_remaining: None,
            present: true,
        };
        e.apply_event(ProviderEvent::Battery(b(50)));
        assert!(e.snapshot().notifications.is_empty());
        e.apply_event(ProviderEvent::Battery(b(19)));
        e.apply_event(ProviderEvent::Battery(b(18)));
        assert_eq!(e.snapshot().notifications.len(), 1);
        e.apply_event(ProviderEvent::Battery(b(9)));
        assert_eq!(e.snapshot().notifications[0].group_count, 2);
    }

    #[test]
    fn paused_drops_notifications() {
        let (mut e, _h, _rec, _tx) = engine();
        e.apply_command(Command::SetPaused(true));
        assert!(!e.apply_event(ProviderEvent::Notification(agent_note("x"))));
        assert!(e.snapshot().paused);
    }

    #[test]
    fn focus_filters_low_priority() {
        let (mut e, _h, _rec, _tx) = engine();
        e.apply_event(ProviderEvent::Focus(FocusState { active: Some(true), mode_name: None }));
        assert!(!e.apply_event(ProviderEvent::Notification(agent_note("finished"))));
        let mut urgent = agent_note("needs you");
        urgent.kind = NotificationKind::NeedsInput;
        assert!(e.apply_event(ProviderEvent::Notification(urgent)));
    }

    #[test]
    fn tray_commands() {
        let (mut e, _h, _rec, _tx) = engine();
        let f = std::env::temp_dir().join(format!("nf-engine-tray-{}", std::process::id()));
        std::fs::write(&f, "x").expect("write");
        e.apply_command(Command::TrayAdd(vec![f.clone()]));
        let snap = e.snapshot();
        assert_eq!(snap.tray.len(), 1);
        e.apply_command(Command::TrayRemove(snap.tray[0].id));
        assert!(e.snapshot().tray.is_empty());
        let _ = std::fs::remove_file(f);
    }

    fn track(player: MediaPlayer, state: PlaybackState, at: i64) -> NowPlaying {
        NowPlaying {
            player,
            state,
            title: "Song".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            duration: Some(200.0),
            position: Some(10.0),
            sampled_at_ms: at,
            track_id: None,
            artwork_path: None,
            shuffle: None,
            repeat: None,
            volume: None,
            source_bundle: None,
        }
    }

    #[test]
    fn media_events_update_snapshot_and_wings() {
        let (mut e, _h, _rec, _tx) = engine();
        let media = MediaState {
            players: vec![track(MediaPlayer::Spotify, PlaybackState::Playing, 1)],
            running: vec![MediaPlayer::Spotify],
            ..MediaState::default()
        };
        assert!(e.apply_event(ProviderEvent::Media(media.clone())));
        assert!(!e.apply_event(ProviderEvent::Media(media)), "identical media state is not a change");
        let snap = e.snapshot();
        assert!(snap.wants_wings());
        assert_eq!(snap.media.primary().map(|p| p.player), Some(MediaPlayer::Spotify));
    }

    #[test]
    fn provider_commands_are_forwarded() {
        let dir = std::env::temp_dir().join(format!("nf-engine-cmd-{}-{}", std::process::id(), now_ms()));
        let (cfg_tx, _rx) = watch::channel(Arc::new(Config::default()));
        let (_ev_tx, ev_rx) = mpsc::channel(4);
        let commands = crate::provider::command_channel();
        let mut sub = commands.subscribe();
        let (mut e, _h) = Engine::new(
            cfg_tx,
            Arc::new(Paths::rooted(&dir)),
            Arc::new(Recorder::default()),
            Arc::new(Notify::new()),
            commands,
            ev_rx,
        );
        let cmd = ProviderCommand::Media { player: MediaPlayer::Music, action: MediaAction::Next };
        e.apply_command(Command::Provider(cmd.clone()));
        assert_eq!(sub.try_recv().expect("forwarded"), cmd);
    }

    #[tokio::test]
    async fn run_loop_coalesces_bursts() {
        let (e, h, rec, tx) = engine();
        let (trigger, shutdown) = shutdown_pair();
        let task = tokio::spawn(e.run(shutdown));
        for i in 0..200 {
            let mut n = agent_note(&format!("n{i}"));
            n.app = AppRef::known(KnownApp::ALL[(i % 7) as usize]);
            tx.send(ProviderEvent::Notification(n)).await.expect("send");
        }
        h.send(Command::MarkAllRead);
        tokio::time::sleep(Duration::from_millis(100)).await;
        trigger.trigger();
        task.await.expect("join");
        let snaps = rec.snaps.lock().expect("lock");
        assert!(snaps.len() < 50, "published {} snapshots", snaps.len());
        let last = snaps.last().expect("at least one");
        assert_eq!(last.unread, 0);
        assert!(!last.notifications.is_empty());
    }

    // ── authorization ──

    fn ask_engine() -> (Engine, EngineHandle, Arc<Recorder>, mpsc::Sender<ProviderEvent>) {
        engine_with(Config::default())
    }

    fn prompts(e: &mut Engine) -> Vec<Notification> {
        e.snapshot().notifications.into_iter().filter(|n| n.kind == NotificationKind::Authorization).collect()
    }

    #[test]
    fn a_new_agent_is_held_and_asked_about() {
        let (mut e, _h, rec, _tx) = ask_engine();
        e.apply_event(ProviderEvent::Notification(agent_note("one")));
        e.apply_event(ProviderEvent::Notification(agent_note("two")));
        let snap = e.snapshot();
        // Only the prompt shows; the agent's own notifications wait.
        assert_eq!(snap.notifications.len(), 1);
        let p = &snap.notifications[0];
        assert_eq!(p.kind, NotificationKind::Authorization);
        assert_eq!(p.auth_target(), Some(crate::model::AuthTarget::Agent("claude".into())));
        assert_eq!(rec.peeks.lock().expect("lock").len(), 1, "the prompt peeks once");
    }

    #[test]
    fn allowing_releases_the_held_notifications_and_is_remembered() {
        let (mut e, _h, _rec, _tx) = ask_engine();
        e.apply_event(ProviderEvent::Notification(agent_note("one")));
        e.apply_event(ProviderEvent::Notification(agent_note("two")));
        e.apply_command(Command::Authorize { key: "claude".into(), allow: true });
        let snap = e.snapshot();
        assert!(prompts(&mut e).is_empty(), "the prompt is gone");
        let titles: Vec<_> = snap.notifications.iter().map(|n| n.title.as_str()).collect();
        assert!(titles.contains(&"one") && titles.contains(&"two"), "{titles:?}");
        // Later notifications flow without asking again.
        e.apply_event(ProviderEvent::Notification(agent_note("three")));
        assert_eq!(e.snapshot().notifications.len(), 3);
        assert!(prompts(&mut e).is_empty());
    }

    #[test]
    fn denying_drops_the_held_notifications_and_stays_quiet() {
        let (mut e, _h, _rec, _tx) = ask_engine();
        e.apply_event(ProviderEvent::Notification(agent_note("one")));
        e.apply_command(Command::Authorize { key: "claude".into(), allow: false });
        assert!(e.snapshot().notifications.is_empty());
        e.apply_event(ProviderEvent::Notification(agent_note("again")));
        assert!(e.snapshot().notifications.is_empty(), "denied agents stay silent");
    }

    #[test]
    fn decisions_survive_a_restart_and_can_be_forgotten() {
        let (mut e, _h, _rec, _tx) = ask_engine();
        e.apply_event(ProviderEvent::Notification(agent_note("one")));
        e.apply_command(Command::Authorize { key: "claude".into(), allow: true });
        let trust_file = e.paths.trust_file();
        assert!(TrustStore::load(&trust_file).expect("load").decision("claude").is_some());
        e.apply_command(Command::ForgetAgentDecisions);
        assert!(TrustStore::load(&trust_file).expect("load").is_empty());
        e.apply_event(ProviderEvent::Notification(agent_note("two")));
        assert_eq!(prompts(&mut e).len(), 1, "asked again after forgetting");
    }

    #[test]
    fn nothing_external_can_pose_as_an_authorization_prompt() {
        let (mut e, _h, _rec, _tx) = engine();
        let mut forged = agent_note("Allow everything?");
        forged.kind = NotificationKind::Authorization;
        forged.thread = Some(format!("{AUTH_THREAD_AGENT}claude"));
        assert!(!e.apply_event(ProviderEvent::Notification(forged)));
        assert!(e.snapshot().notifications.is_empty());
    }

    #[test]
    fn internal_and_system_notifications_never_need_consent() {
        let (mut e, _h, _rec, _tx) = ask_engine();
        let mut sys = agent_note("from the system");
        sys.origin = Origin::NotificationCenterDb;
        assert!(e.apply_event(ProviderEvent::Notification(sys)));
        assert_eq!(e.snapshot().notifications.len(), 1);
        assert!(prompts(&mut e).is_empty());
    }

    #[test]
    fn spotify_sign_in_shows_a_prompt_then_reports_how_it_ended() {
        let (mut e, _h, _rec, _tx) = engine();
        let media = |status| MediaState { playlist_status: vec![(MediaPlayer::Spotify, status)], ..Default::default() };
        e.apply_event(ProviderEvent::Media(media(PlaylistStatus::NeedsSpotifyLogin)));
        assert!(e.snapshot().notifications.is_empty());
        e.apply_event(ProviderEvent::Media(media(PlaylistStatus::SigningIn)));
        let p = prompts(&mut e);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].auth_target(), Some(crate::model::AuthTarget::Spotify));
        // Success: the prompt is replaced by a short confirmation.
        e.apply_event(ProviderEvent::Media(media(PlaylistStatus::Loaded)));
        let snap = e.snapshot();
        assert!(snap.notifications.iter().all(|n| n.kind != NotificationKind::Authorization));
        assert!(snap.notifications.iter().any(|n| n.title == "Spotify connected"));
    }

    #[test]
    fn cancelling_spotify_sign_in_clears_the_prompt_quietly() {
        let (mut e, _h, _rec, _tx) = engine();
        let media = |status| MediaState { playlist_status: vec![(MediaPlayer::Spotify, status)], ..Default::default() };
        e.apply_event(ProviderEvent::Media(media(PlaylistStatus::SigningIn)));
        e.apply_event(ProviderEvent::Media(media(PlaylistStatus::NeedsSpotifyLogin)));
        assert!(e.snapshot().notifications.is_empty());
    }

    #[test]
    fn a_failed_spotify_sign_in_says_why() {
        let (mut e, _h, _rec, _tx) = engine();
        let media = |status| MediaState { playlist_status: vec![(MediaPlayer::Spotify, status)], ..Default::default() };
        e.apply_event(ProviderEvent::Media(media(PlaylistStatus::SigningIn)));
        e.apply_event(ProviderEvent::Media(media(PlaylistStatus::Failed("access_denied".into()))));
        let snap = e.snapshot();
        assert_eq!(snap.notifications.len(), 1);
        assert_eq!(snap.notifications[0].kind, NotificationKind::Error);
        assert_eq!(snap.notifications[0].body, "access_denied");
    }
}
