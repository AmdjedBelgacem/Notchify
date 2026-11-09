//! The engine: single owner of application state.
//!
//! Providers send [`ProviderEvent`]s, the UI sends [`Command`]s; the engine
//! applies both to [`AppState`] and publishes immutable [`Snapshot`]s to a
//! [`UiSink`]. Snapshot publication is coalesced (at most one per
//! [`COALESCE`]) so a burst of a hundred notifications produces a handful of
//! UI updates, never a hundred.

use crate::apps::AppRef;
use crate::calls::{
    ACTIVE_GONE_MS, ActiveCall, Call, CallId, CallKind, ENDED_QUIET_MS, RING_TIMEOUT_MS, Recognised, UNKNOWN_CALLER,
    WINDOW_GONE_MS, recognise, same_call,
};
use crate::config::{Config, Paths};
use crate::model::{
    AUTH_THREAD_AGENT, AUTH_THREAD_PERMISSION, AUTH_THREAD_SPOTIFY, AgentLimits, AgentUsage, AudioState, BatteryStatus,
    CalendarState, DeviceBattery, FocusState, MediaPlayer, MediaState, NewNotification, Notification, NotificationId,
    NotificationKind, Origin, PlaylistStatus, Priority, ProviderStatus, TimerState, TrayItem, TrayItemId, UsageAgent,
    Weather,
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
    /// Hide a notification and bring it back after a snooze.
    SnoozeNotification(NotificationId),
    /// A held tool-permission request was answered in the notch: drop its prompt.
    ResolvePermission(String),
    /// Dismiss every notification from an app (by rule key).
    DismissApp(String),
    ClearNotifications,
    MarkAllRead,
    TrayAdd(Vec<PathBuf>),
    /// Shelve the clipboard's text (or link) into the tray.
    TrayAddText(Box<String>),
    /// Pin or unpin a tray item (pinned ones never age out).
    TrayTogglePin(TrayItemId),
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
    /// A call was answered or declined: stop showing it.
    EndCall(CallId),
    /// Show this call first.
    PromoteCall(CallId),
    /// Calls that stopped ringing become missed calls.
    ExpireCalls,
    /// A ringing call was picked up from the notch: keep showing it.
    CallAnswered(CallId),
    /// Answer was pressed; it's picked up once the app's button can be pressed.
    CallAnswering(CallId),
    /// The app's own answer button was pressed, in this process.
    CallAnsweredIn(CallId, i32),
    /// An app shows a hang-up button: a call ringing in it was picked up there,
    /// and a call in progress is still going.
    WindowInCall {
        pid: i32,
    },
    /// The app with the call in progress is still making sound.
    ActiveCallSound,
    /// The call in progress is over.
    EndActiveCall,
    /// Ring a sample call, so the call view can be tried.
    TestCall,
    /// An app is ringing in its own window, or still is.
    WindowCall {
        pid: i32,
        bundle_id: String,
        source: String,
        caller: String,
        kind: CallKind,
    },
    /// Start a user countdown timer.
    StartTimer {
        duration_ms: i64,
        label: String,
    },
    /// A pomodoro cycle: focus and break lengths that roll into each other.
    StartPomodoro { work_ms: i64, break_ms: i64 },
    /// Cancel the running timer.
    CancelTimer,
}

/// Immutable view of the state handed to the UI.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub generation: u64,
    pub now_ms: i64,
    pub battery: Option<BatteryStatus>,
    pub devices: Vec<DeviceBattery>,
    pub weather: Option<Weather>,
    pub calendar: CalendarState,
    pub timer: Option<TimerState>,
    pub focus: FocusState,
    pub audio: AudioState,
    pub media: MediaState,
    pub usage: AgentUsage,
    /// Ringing now, the one shown first at the front.
    pub calls: Vec<Call>,
    /// The call you picked up.
    pub active_call: Option<ActiveCall>,
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
            || !self.calls.is_empty()
            || self.active_call.is_some()
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
    usage: AgentUsage,
    calls: Vec<Call>,
    active_call: Option<ActiveCall>,
    next_call: u64,
    /// When each recent call ended, so late copies of it don't ring again.
    ended_calls: BTreeMap<String, i64>,
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
    /// Antigravity's quota tile, owned by its own provider (the usage
    /// provider's snapshots don't carry it).
    antigravity_limits: Option<AgentLimits>,
    /// Calendar accounts and upcoming events, owned by the calendar provider.
    calendar: CalendarState,
    /// A user-started countdown timer.
    timer: Option<TimerState>,
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
    /// Snoozed notifications: (due at, the notification to bring back).
    snoozed: Vec<(i64, NewNotification)>,
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
            snoozed: Vec::new(),
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
            // A running timer wakes the engine when it ends; so does the
            // earliest snoozed notification.
            let timer_end = self
                .state
                .timer
                .as_ref()
                .filter(|t| !t.expired)
                .map(|t| std::time::Duration::from_millis((t.ends_at_ms - now_ms()).max(50) as u64));
            let snooze_end = self
                .snoozed
                .first()
                .map(|(due, _)| std::time::Duration::from_millis((due - now_ms()).max(50) as u64));
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
                _ = async {
                    match timer_end {
                        Some(d) => tokio::time::sleep(d).await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let Some(t) = self.state.timer.as_mut().filter(|t| !t.expired) {
                        let mut phase_note = None;
                        // A pomodoro never sits expired: it rolls into the
                        // next phase and keeps counting.
                        if let Some(c) = t.pomodoro.clone() {
                            let (next_work, round, title, body) = if t.phase_work {
                                let mins = c.break_ms / 60_000;
                                (
                                    false,
                                    t.round,
                                    format!("Focus round {} done", t.round + 1),
                                    format!("Take {mins} min — the break is running."),
                                )
                            } else {
                                let mins = c.work_ms / 60_000;
                                (
                                    true,
                                    t.round + 1,
                                    format!("Break over — round {}", t.round + 2),
                                    format!("Back to focus for {mins} min."),
                                )
                            };
                            let (duration, label) = if next_work {
                                (c.work_ms, format!("Focus {}", round + 1))
                            } else {
                                (c.break_ms, "Break".into())
                            };
                            *t = TimerState {
                                label,
                                ends_at_ms: now_ms() + duration,
                                duration_ms: duration,
                                expired: false,
                                pomodoro: Some(c),
                                phase_work: next_work,
                                round,
                            };
                            phase_note = Some((title, body));
                        } else {
                            t.expired = true;
                        }
                        dirty = true;
                        let label = t.label.clone();
                        let mins = t.duration_ms / 60_000;
                        let summary = if mins >= 1 { format!("{mins} min") } else { "timer".into() };
                        let is_pomodoro = phase_note.is_some();
                        let (title, body) =
                            phase_note.take().unwrap_or((format!("Timer done — {summary}"), String::new()));
                        self.ingest(NewNotification {
                            app: crate::apps::AppRef {
                                known: None,
                                bundle_id: None,
                                display_name: if is_pomodoro { "Pomodoro" } else { "Timer" }.into(),
                                platform: None,
                            },
                            origin: Origin::Internal,
                            kind: NotificationKind::TaskFinished,
                            priority: Some(Priority::High),
                            title,
                            subtitle: Some(label),
                            body,
                            timestamp_ms: now_ms(),
                            thread: None,
                            url: None,
                            tokens: None,
                        });
                    }
                }
                _ = async {
                    match snooze_end {
                        Some(d) => tokio::time::sleep(d).await,
                        None => std::future::pending().await,
                    }
                } => {
                    let now = now_ms();
                    let mut woken = Vec::new();
                    self.snoozed.retain(|(due, n)| {
                        if *due <= now + 50 {
                            woken.push(n.clone());
                            false
                        } else {
                            true
                        }
                    });
                    for n in woken {
                        self.ingest(n);
                    }
                    dirty = true;
                }
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
            calendar: s.calendar.clone(),
            timer: s.timer.clone(),
            focus: s.focus.clone(),
            audio: s.audio.clone(),
            media: s.media.clone(),
            usage: {
                {
                    let mut usage = s.usage.clone();
                    usage.limits.retain(|l| l.agent != UsageAgent::Antigravity);
                    if let Some(a) = &s.antigravity_limits {
                        {
                            usage.limits.push(a.clone());
                        }
                    }
                    usage
                }
            },
            calls: s.calls.clone(),
            active_call: s.active_call.clone(),
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
            ProviderEvent::AgentHook(h) => self.agent_hook(h),
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
            ProviderEvent::Limits(l) => {
                let changed = self.state.antigravity_limits.as_ref() != Some(&l);
                self.state.antigravity_limits = Some(l);
                changed
            }
            ProviderEvent::Calendar(c) => {
                let changed = self.state.calendar != c;
                self.state.calendar = c;
                changed
            }
            ProviderEvent::Usage(u) => {
                let changed = self.state.usage != u;
                self.state.usage = u;
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
            Command::ResolvePermission(id) => match self.prompt_id(&format!("{AUTH_THREAD_PERMISSION}{id}")) {
                Some(n) => self.state.pipeline.dismiss(n),
                None => false,
            },
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
            Command::TrayTogglePin(id) => {
                let changed = self.state.tray.toggle_pin(id);
                self.persist_tray();
                changed
            }
            Command::TrayAddText(text) => {
                let changed = self.state.tray.add_text(*text, cfg.tray.max_items, now_ms());
                self.persist_tray();
                changed
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
            Command::StartTimer { duration_ms, label } => {
                let ends_at_ms = now_ms() + duration_ms;
                self.state.timer =
                    Some(TimerState { label, ends_at_ms, duration_ms, expired: false, ..Default::default() });
                true
            }
            Command::StartPomodoro { work_ms, break_ms } => {
                let ends_at_ms = now_ms() + work_ms;
                self.state.timer = Some(TimerState {
                    label: "Focus 1".into(),
                    ends_at_ms,
                    duration_ms: work_ms,
                    expired: false,
                    pomodoro: Some(crate::model::PomodoroCycle { work_ms, break_ms }),
                    phase_work: true,
                    round: 0,
                });
                true
            }
            Command::CancelTimer => self.state.timer.take().is_some(),
            Command::SnoozeNotification(id) => {
                // Dismiss now, re-ingest the same content when the snooze is
                // up (the pipeline re-ranks and re-peeks it like anything new).
                const SNOOZE_MS: i64 = 30 * 60_000;
                let Some(n) = self.state.pipeline.get(id).cloned() else { return false };
                if !self.state.pipeline.dismiss(id) {
                    return false;
                }
                let due = now_ms() + SNOOZE_MS;
                let again = NewNotification {
                    app: n.app,
                    origin: n.origin,
                    kind: n.kind,
                    priority: Some(n.priority),
                    title: n.title,
                    subtitle: n.subtitle,
                    body: n.body,
                    timestamp_ms: n.timestamp_ms,
                    thread: n.thread,
                    url: n.url,
                    tokens: None,
                };
                self.snoozed.push((due, again));
                true
            }
            Command::Provider(c) => {
                // No receivers just means the relevant provider is disabled.
                let _ = self.provider_commands.send(c);
                false
            }
            Command::Authorize { key, allow } => self.authorize(&key, allow),
            Command::EndCall(id) => self.end_call(id),
            Command::PromoteCall(id) => {
                let Some(i) = self.state.calls.iter().position(|c| c.id == id) else { return false };
                self.state.calls[..=i].rotate_right(1);
                true
            }
            Command::ExpireCalls => self.expire_calls(now_ms()),
            Command::CallAnswered(id) => self.pick_up(id, now_ms()),
            Command::CallAnswering(id) => match self.state.calls.iter_mut().find(|c| c.id == id) {
                Some(c) => {
                    c.answering = true;
                    c.last_seen_ms = now_ms();
                    true
                }
                None => false,
            },
            Command::CallAnsweredIn(id, pid) => {
                if let Some(c) = self.state.calls.iter_mut().find(|c| c.id == id) {
                    c.window_pid = Some(pid);
                }
                self.pick_up(id, now_ms())
            }
            Command::WindowInCall { pid } => {
                let now = now_ms();
                if let Some(a) = self.state.active_call.as_mut().filter(|a| a.window_pid == Some(pid)) {
                    a.last_seen_ms = now;
                    return false;
                }
                match self.state.calls.iter().find(|c| c.window_pid == Some(pid)).map(|c| c.id) {
                    Some(id) => self.pick_up(id, now),
                    None => false,
                }
            }
            Command::ActiveCallSound => {
                if let Some(a) = self.state.active_call.as_mut() {
                    a.last_seen_ms = now_ms();
                }
                false
            }
            Command::EndActiveCall => self.state.active_call.take().is_some(),
            Command::TestCall => {
                let app = AppRef { known: None, bundle_id: None, display_name: "Notchisland".into(), platform: None };
                let seed =
                    Recognised { caller: "Test Caller".into(), kind: CallKind::Audio, source: "Notchisland".into() };
                // Asked for on purpose, so it isn't a late copy of the last test.
                self.state.ended_calls.clear();
                self.ring(seed, app, None, String::new(), None, now_ms())
            }
            Command::WindowCall { pid, bundle_id, source, caller, kind } => {
                if !self.config().notifications.incoming_calls {
                    return false;
                }
                let app =
                    AppRef { known: None, bundle_id: Some(bundle_id), display_name: source.clone(), platform: None };
                self.ring(Recognised { caller, kind, source }, app, None, String::new(), Some(pid), now_ms())
            }
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
            tokens: None,
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
        let spotify = AppRef {
            known: None,
            bundle_id: Some("com.spotify.client".into()),
            display_name: "Spotify".into(),
            platform: None,
        };
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
            tokens: None,
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
    /// Start ringing, or note that a call already ringing is still going.
    fn ring(
        &mut self,
        seed: Recognised,
        app: AppRef,
        url: Option<String>,
        banner_title: String,
        window_pid: Option<i32>,
        now: i64,
    ) -> bool {
        let call = Call {
            id: CallId(self.state.next_call),
            caller: seed.caller,
            kind: seed.kind,
            source: seed.source,
            app,
            url,
            banner_title,
            window_pid,
            last_seen_ms: now,
            answering: false,
        };
        let key = call.key();
        if self.state.ended_calls.iter().any(|(k, at)| now - at < ENDED_QUIET_MS && same_call(k, &key)) {
            return false;
        }
        if let Some(c) = self.state.calls.iter_mut().find(|c| same_call(&c.key(), &key)) {
            c.last_seen_ms = now;
            // The notification and the window are the same call; keep the buttons.
            c.window_pid = c.window_pid.or(window_pid);
            if c.caller == UNKNOWN_CALLER && call.caller != UNKNOWN_CALLER {
                c.caller = call.caller;
                return true;
            }
            return false;
        }
        self.state.next_call += 1;
        self.state.calls.push(call);
        true
    }

    /// A ringing call was picked up: it stops ringing and starts its clock.
    fn pick_up(&mut self, id: CallId, now: i64) -> bool {
        let Some(call) = self.state.calls.iter().find(|c| c.id == id).cloned() else { return false };
        self.end_call(id);
        self.state.active_call = Some(ActiveCall {
            caller: call.caller,
            kind: call.kind,
            source: call.source,
            app: call.app,
            window_pid: call.window_pid,
            started_ms: now,
            last_seen_ms: now,
        });
        true
    }

    fn end_call(&mut self, id: CallId) -> bool {
        let Some(i) = self.state.calls.iter().position(|c| c.id == id) else { return false };
        let call = self.state.calls.remove(i);
        let now = now_ms();
        self.state.ended_calls.retain(|_, at| now - *at < ENDED_QUIET_MS);
        self.state.ended_calls.insert(call.key(), now);
        true
    }

    /// Calls nothing has confirmed for a while stopped ringing unanswered.
    fn expire_calls(&mut self, now: i64) -> bool {
        // A notification's call keeps ringing while its tab is out of view.
        let gone: Vec<Call> = self
            .state
            .calls
            .iter()
            .filter(|c| {
                let watched = c.window_pid.is_some() && (c.answering || c.banner_title.is_empty());
                now - c.last_seen_ms >= if watched { WINDOW_GONE_MS } else { RING_TIMEOUT_MS }
            })
            .cloned()
            .collect();
        for call in &gone {
            // Its buttons went while the app kept making sound: it was picked up
            // there. A browser may just be playing something in another tab.
            let picked_up = call.window_pid.is_some()
                && call.app.bundle_id.as_deref().is_some_and(|b| {
                    (call.answering || !crate::apps::is_browser(b))
                        && self.state.audio.sources.iter().any(|s| s.bundle_id.as_deref() == Some(b))
                });
            if picked_up {
                self.pick_up(call.id, now);
                continue;
            }
            self.end_call(call.id);
            self.ingest_now(NewNotification {
                app: call.app.clone(),
                origin: Origin::Internal,
                kind: NotificationKind::Info,
                priority: Some(Priority::High),
                title: "Missed call".into(),
                subtitle: None,
                body: format!("{} · {}", call.caller, call.source),
                timestamp_ms: now,
                thread: None,
                url: call.url.clone(),
                tokens: None,
            });
        }
        // A sample call has no app to watch, so it lasts until it's hung up.
        let call_over = self
            .state
            .active_call
            .as_ref()
            .is_some_and(|a| a.app.bundle_id.is_some() && now - a.last_seen_ms >= ACTIVE_GONE_MS);
        if call_over {
            self.state.active_call = None;
        }
        !gone.is_empty() || call_over
    }

    /// A coding agent's lifecycle hook. Permission requests become a prompt
    /// raised here — never by the transport, so nothing outside can pose as
    /// one — and every event goes on to the provider tracking live sessions.
    fn agent_hook(&mut self, h: crate::model::AgentHook) -> bool {
        let mut changed = false;
        match (h.event.as_str(), h.permission_id.as_deref()) {
            ("PermissionRequest", Some(id)) => {
                let app = AppRef::known(
                    match h.agent {
                        UsageAgent::Codex => crate::apps::KnownApp::from_key("chatgpt"),
                        UsageAgent::Gemini => crate::apps::KnownApp::from_key("gemini"),
                        _ => crate::apps::KnownApp::from_key("claude"),
                    }
                    .unwrap_or(crate::apps::KnownApp::GenericAgent),
                );
                let denied = self.state.trust.decision(&app.rule_key()) == Some(Decision::Deny);
                if !denied {
                    let project = h.cwd.as_deref().and_then(|c| c.rsplit('/').find(|p| !p.is_empty()));
                    let tool = h.tool.clone().unwrap_or_else(|| "a tool".into());
                    let prompt = NewNotification {
                        title: format!("{} wants to use {tool}", app.display_name),
                        subtitle: project.map(str::to_owned),
                        body: h.detail.clone().unwrap_or_default(),
                        app,
                        origin: Origin::Internal,
                        kind: NotificationKind::Authorization,
                        priority: Some(Priority::High),
                        timestamp_ms: now_ms(),
                        thread: Some(format!("{AUTH_THREAD_PERMISSION}{id}")),
                        url: None,
                        tokens: None,
                    };
                    changed = self.ingest_now(prompt);
                }
            }
            ("PermissionResolved", Some(id)) => {
                if let Some(n) = self.prompt_id(&format!("{AUTH_THREAD_PERMISSION}{id}")) {
                    changed = self.state.pipeline.dismiss(n);
                }
                return changed;
            }
            _ => {}
        }
        let _ = self.provider_commands.send(ProviderCommand::AgentHook(h));
        changed
    }

    fn ingest(&mut self, n: NewNotification) -> bool {
        // Token usage reported along with an event is telemetry, not a message
        // the notch should show: route it to the usage provider instead.
        if let Some(usage) = &n.tokens {
            if let Some(agent) = UsageAgent::from_source(&n.app.display_name) {
                tracing::info!(agent = agent.display_name(), model = ?usage.model, counts_input = usage.counts.input, counts_output = usage.counts.output, "usage reported by an agent");
                let _ = self.provider_commands.send(ProviderCommand::Usage {
                    agent,
                    model: usage.model.clone(),
                    session: n.thread.clone(),
                    counts: usage.counts,
                    at_ms: n.timestamp_ms,
                });
                return false;
            }
            tracing::debug!(source = %n.app.display_name, "usage reported for an unknown agent; ignoring");
            return false;
        }
        if n.kind == NotificationKind::Authorization && n.origin != Origin::Internal {
            tracing::warn!(origin = ?n.origin, "dropped a forged authorization prompt");
            return false;
        }
        if matches!(n.origin, Origin::NotificationCenterDb | Origin::AccessibilityBanner)
            && self.config().notifications.incoming_calls
        {
            let seed = recognise(&n.app, &n.title, n.subtitle.as_deref(), &n.body);
            if let Some(seed) = seed {
                let (app, url, title) = (n.app, n.url, n.title);
                return self.ring(seed, app, url, title, None, now_ms());
            }
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
            app: AppRef { known: None, bundle_id: None, display_name: "Notchisland".into(), platform: None },
            origin: Origin::Internal,
            kind: NotificationKind::Info,
            priority: Some(priority),
            title,
            subtitle: None,
            body,
            timestamp_ms: now_ms(),
            thread: None,
            url: None,
            tokens: None,
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
            tokens: None,
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

    #[test]
    fn snooze_hides_then_reschedules() {
        let (mut e, _h, rec, _tx) = engine();
        e.apply_event(ProviderEvent::Notification(agent_note("later")));
        let id = e.snapshot().notifications[0].id;
        assert!(e.apply_command(Command::SnoozeNotification(id)));
        // Hidden right away.
        assert!(e.snapshot().notifications.is_empty());
        // And queued: the engine wakes for it after the snooze.
        assert_eq!(e.snoozed.len(), 1);
        assert_eq!(e.snoozed[0].1.title, "later");
        assert!(e.snoozed[0].0 > now_ms());
        // An unknown id is a no-op.
        assert!(!e.apply_command(Command::SnoozeNotification(NotificationId(99_999))));
        assert_eq!(e.snoozed.len(), 1);
        let _ = rec;
    }

    #[test]
    fn pomodoro_starts_in_focus() {
        let (mut e, _h, _rec, _tx) = engine();
        assert!(e.apply_command(Command::StartPomodoro { work_ms: 25 * 60_000, break_ms: 5 * 60_000 }));
        let t = e.snapshot().timer.expect("timer");
        assert_eq!(t.label, "Focus 1");
        assert!(t.pomodoro.is_some());
        assert!(t.phase_work);
        assert_eq!(t.round, 0);
        // Cancel stops the whole cycle.
        assert!(e.apply_command(Command::CancelTimer));
        assert!(e.snapshot().timer.is_none());
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
    fn reported_tokens_reach_the_tracker_and_show_nothing() {
        // Own the command receiver so the broadcast can be inspected.
        let mut cfg = Config::default();
        cfg.agents.require_approval = false;
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("nf-engine-usage-{}-{n}", now_ms()));
        let paths = Arc::new(Paths::rooted(&dir));
        let (cfg_tx, _rx) = watch::channel(Arc::new(cfg));
        let rec = Arc::new(Recorder::default());
        let (_ev_tx, ev_rx) = mpsc::channel(1024);
        let (cmd_tx, mut cmd_rx) = tokio::sync::broadcast::channel(crate::provider::COMMAND_CAPACITY);
        let (mut e, _h) = Engine::new(cfg_tx, paths, rec.clone(), Arc::new(Notify::new()), cmd_tx, ev_rx);
        let note = NewNotification {
            app: AppRef {
                known: Some(KnownApp::GenericAgent),
                bundle_id: None,
                display_name: "gemini".into(),
                platform: None,
            },
            origin: Origin::AgentWebhook,
            kind: NotificationKind::Info,
            priority: None,
            title: "turn".into(),
            subtitle: None,
            body: String::new(),
            timestamp_ms: now_ms(),
            thread: Some("sess-9".into()),
            url: None,
            tokens: Some(crate::model::ReportedUsage {
                counts: crate::model::TokenCounts { input: 1200, output: 340, ..Default::default() },
                model: Some("gemini-3-flash".into()),
            }),
        };
        e.apply_event(ProviderEvent::Notification(note));
        assert_eq!(e.snapshot().notifications.len(), 0, "usage is telemetry, not a message");
        let cmd = cmd_rx.try_recv().expect("the usage command was broadcast");
        match cmd {
            crate::provider::ProviderCommand::Usage { agent, model, session, counts, .. } => {
                assert_eq!(agent, UsageAgent::Gemini);
                assert_eq!(model.as_deref(), Some("gemini-3-flash"));
                assert_eq!(session.as_deref(), Some("sess-9"));
                assert_eq!(counts.input, 1200);
            }
            other => panic!("expected a Usage command, got {other:?}"),
        }
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

    fn call_note(title: &str, body: &str) -> NewNotification {
        NewNotification {
            app: AppRef {
                known: None,
                bundle_id: Some("net.whatsapp.WhatsApp".into()),
                display_name: "WhatsApp".into(),
                platform: None,
            },
            origin: Origin::NotificationCenterDb,
            kind: NotificationKind::Message,
            priority: None,
            title: title.into(),
            subtitle: None,
            body: body.into(),
            timestamp_ms: now_ms(),
            thread: None,
            url: None,
            tokens: None,
        }
    }

    #[test]
    fn a_call_rings_instead_of_notifying() {
        let (mut e, _h, _r, _tx) = engine();
        assert!(e.apply_event(ProviderEvent::Notification(call_note("Léa", "Incoming video call"))));
        let snap = e.snapshot();
        assert_eq!(snap.calls.len(), 1);
        assert_eq!(snap.calls[0].caller, "Léa");
        assert!(snap.notifications.is_empty());
        assert!(snap.wants_wings());
        // Its second copy isn't a second call.
        e.apply_event(ProviderEvent::Notification(call_note("Léa", "Incoming video call")));
        assert_eq!(e.snapshot().calls.len(), 1);
        // An ordinary message is still a message.
        e.apply_event(ProviderEvent::Notification(call_note("Léa", "call me later")));
        assert_eq!(e.snapshot().notifications.len(), 1);
    }

    #[test]
    fn several_calls_stack_and_can_be_reordered() {
        let (mut e, _h, _r, _tx) = engine();
        for who in ["A", "B", "C"] {
            e.apply_event(ProviderEvent::Notification(call_note(who, "Incoming call")));
        }
        let ids: Vec<_> = e.snapshot().calls.iter().map(|c| c.id).collect();
        e.apply_command(Command::PromoteCall(ids[2]));
        let order: Vec<_> = e.snapshot().calls.iter().map(|c| c.caller.clone()).collect();
        assert_eq!(order, ["C", "A", "B"]);
        e.apply_command(Command::EndCall(ids[2]));
        assert_eq!(e.snapshot().calls.len(), 2);
        // A late copy of the ended call doesn't ring again.
        e.apply_event(ProviderEvent::Notification(call_note("C", "Incoming call")));
        assert_eq!(e.snapshot().calls.len(), 2);
    }

    #[test]
    fn an_unanswered_call_becomes_a_missed_call() {
        let (mut e, _h, _r, _tx) = engine();
        e.apply_event(ProviderEvent::Notification(call_note("Léa", "Incoming call")));
        assert!(!e.expire_calls(now_ms() + 1000), "still ringing");
        assert!(e.expire_calls(now_ms() + RING_TIMEOUT_MS + 1000));
        let snap = e.snapshot();
        assert!(snap.calls.is_empty());
        assert_eq!(snap.notifications[0].title, "Missed call");
        assert!(snap.notifications[0].body.contains("Léa"));
    }

    #[test]
    fn a_ringing_window_is_a_call_until_its_buttons_go() {
        let (mut e, _h, _r, _tx) = engine();
        let ring = || Command::WindowCall {
            pid: 42,
            bundle_id: "us.zoom.xos".into(),
            source: "Zoom".into(),
            caller: "Sam".into(),
            kind: CallKind::Video,
        };
        e.apply_command(ring());
        e.apply_command(ring());
        let snap = e.snapshot();
        assert_eq!(snap.calls.len(), 1);
        assert_eq!(snap.calls[0].window_pid, Some(42));
        assert!(!e.expire_calls(now_ms() + 1000), "still ringing");
        assert!(e.expire_calls(now_ms() + WINDOW_GONE_MS + 1000), "the window went away");
        assert_eq!(e.snapshot().notifications[0].title, "Missed call");
    }

    #[test]
    fn answering_starts_a_call_that_ends_when_its_signs_stop() {
        let (mut e, _h, _r, _tx) = engine();
        let ring = Command::WindowCall {
            pid: 7,
            bundle_id: "com.hnc.Discord".into(),
            source: "Discord".into(),
            caller: "Sam".into(),
            kind: CallKind::Audio,
        };
        e.apply_command(ring);
        let id = e.snapshot().calls[0].id;
        e.apply_command(Command::CallAnswered(id));
        let snap = e.snapshot();
        assert!(snap.calls.is_empty(), "it stopped ringing");
        assert_eq!(snap.active_call.as_ref().map(|a| a.caller.as_str()), Some("Sam"));
        assert!(snap.wants_wings());
        assert!(snap.notifications.is_empty(), "an answered call isn't missed");
        // Its hang-up button keeps it going; without any sign, it ends.
        e.apply_command(Command::WindowInCall { pid: 7 });
        assert!(!e.expire_calls(now_ms() + 1000));
        assert!(e.expire_calls(now_ms() + ACTIVE_GONE_MS + 1000));
        assert!(e.snapshot().active_call.is_none());
    }

    #[test]
    fn a_call_picked_up_in_the_app_is_not_missed() {
        let (mut e, _h, _r, _tx) = engine();
        e.apply_command(Command::WindowCall {
            pid: 9,
            bundle_id: "net.whatsapp.WhatsApp".into(),
            source: "WhatsApp".into(),
            caller: "Léa".into(),
            kind: CallKind::Audio,
        });
        e.apply_command(Command::WindowInCall { pid: 9 });
        let snap = e.snapshot();
        assert!(snap.calls.is_empty());
        assert!(snap.active_call.is_some());
        assert!(snap.notifications.is_empty());
    }

    fn sound_from(e: &mut Engine, bundle: &str) {
        e.state.audio.sources = vec![crate::model::AudioSource {
            pid: 1,
            name: "App".into(),
            bundle_id: Some(bundle.into()),
            app_path: None,
            device_uids: Vec::new(),
        }];
    }

    fn window_call(pid: i32, bundle: &str, caller: &str) -> Command {
        Command::WindowCall {
            pid,
            bundle_id: bundle.into(),
            source: "Messenger in Dia".into(),
            caller: caller.into(),
            kind: CallKind::Audio,
        }
    }

    #[test]
    fn answering_out_of_view_is_not_picked_up_until_the_button_is_pressed() {
        let (mut e, _h, _r, _tx) = engine();
        e.apply_command(window_call(7, "company.thebrowser.dia", "Léa"));
        let id = e.snapshot().calls[0].id;
        e.apply_command(Command::CallAnswering(id));
        let snap = e.snapshot();
        assert!(snap.active_call.is_none(), "still ringing");
        assert!(snap.calls[0].answering);
        e.apply_command(Command::CallAnsweredIn(id, 9));
        let snap = e.snapshot();
        assert!(snap.calls.is_empty());
        assert_eq!(snap.active_call.expect("in a call").window_pid, Some(9));
    }

    #[test]
    fn a_caller_who_cant_be_read_is_not_a_second_call() {
        let (mut e, _h, _r, _tx) = engine();
        let dia = "company.thebrowser.dia";
        e.apply_command(window_call(7, dia, UNKNOWN_CALLER));
        assert!(e.apply_command(window_call(7, dia, "Léa")), "learned the name");
        e.apply_command(window_call(7, dia, UNKNOWN_CALLER));
        let snap = e.snapshot();
        assert_eq!(snap.calls.len(), 1);
        assert_eq!(snap.calls[0].caller, "Léa");
        // Declined, it doesn't come back while its buttons are still fading.
        e.apply_command(Command::EndCall(snap.calls[0].id));
        assert!(!e.apply_command(window_call(7, dia, UNKNOWN_CALLER)));
        assert!(e.snapshot().calls.is_empty());
    }

    #[test]
    fn buttons_gone_with_sound_on_is_a_call_picked_up_in_the_app() {
        let (mut e, _h, _r, _tx) = engine();
        e.apply_command(window_call(7, "com.hnc.Discord", "Léa"));
        sound_from(&mut e, "com.hnc.Discord");
        e.expire_calls(now_ms() + WINDOW_GONE_MS + 1000);
        let snap = e.snapshot();
        assert!(snap.active_call.is_some());
        assert!(snap.notifications.is_empty());
    }

    #[test]
    fn a_browser_making_sound_only_counts_once_answer_was_pressed() {
        let dia = "company.thebrowser.dia";
        let (mut e, _h, _r, _tx) = engine();
        e.apply_command(window_call(7, dia, "Léa"));
        sound_from(&mut e, dia);
        e.expire_calls(now_ms() + WINDOW_GONE_MS + 1000);
        let snap = e.snapshot();
        assert!(snap.active_call.is_none(), "may be a video in another tab");
        assert_eq!(snap.notifications[0].title, "Missed call");

        let (mut e, _h, _r, _tx) = engine();
        e.apply_command(window_call(7, dia, "Léa"));
        let id = e.snapshot().calls[0].id;
        e.apply_command(Command::CallAnswering(id));
        sound_from(&mut e, dia);
        e.expire_calls(now_ms() + WINDOW_GONE_MS + 1000);
        assert!(e.snapshot().active_call.is_some());
    }

    #[test]
    fn hanging_up_ends_the_call() {
        let (mut e, _h, _r, _tx) = engine();
        e.apply_command(Command::TestCall);
        let id = e.snapshot().calls[0].id;
        e.apply_command(Command::CallAnswered(id));
        assert!(!e.expire_calls(now_ms() + ACTIVE_GONE_MS + 1000), "a sample call waits to be hung up");
        assert!(e.apply_command(Command::EndActiveCall));
        assert!(e.snapshot().active_call.is_none());
    }

    #[test]
    fn calls_can_be_switched_off() {
        let (mut e, _h, _r, _tx) = engine();
        let mut cfg = (*e.config()).clone();
        cfg.notifications.incoming_calls = false;
        e.apply_command(Command::UpdateConfig(Box::new(cfg)));
        e.apply_event(ProviderEvent::Notification(call_note("Léa", "Incoming call")));
        let snap = e.snapshot();
        assert!(snap.calls.is_empty());
        assert_eq!(snap.notifications.len(), 1);
    }
}
