//! Provider plug-in system.
//!
//! A *provider* is a long-running async task that observes something (battery,
//! weather, a coding agent, the Notification Center database…) and emits
//! [`ProviderEvent`]s. Providers never touch the UI or shared state; the
//! [`Engine`](crate::engine::Engine) is the only consumer of their events.
//!
//! ## Writing a provider
//!
//! ```ignore
//! struct Clock;
//! impl Provider for Clock {
//!     fn descriptor(&self) -> ProviderDescriptor {
//!         ProviderDescriptor { id: "clock", name: "Clock", permissions: &[] }
//!     }
//!     fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
//!         Box::pin(async move {
//!             ctx.set_status(ProviderStatus::Running).await;
//!             loop {
//!                 // … emit events with ctx.emit(…) …
//!                 if ctx.sleep_or_refresh(Duration::from_secs(60)).await == Wake::Shutdown {
//!                     return Ok(());
//!                 }
//!             }
//!         })
//!     }
//! }
//! ```
//!
//! and register a [`ProviderSpec`] in `notchforge_providers::registry()`.
//! The [`Supervisor`] starts/stops it as the config changes and restarts it
//! with exponential backoff if it fails.

use crate::config::{Config, Paths};
use crate::model::{
    BatteryStatus, Coordinates, DeviceBattery, FocusState, NewNotification, Permission, ProviderStatus, Weather,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, mpsc, watch};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderDescriptor {
    /// Stable identifier, e.g. `"weather"`.
    pub id: &'static str,
    pub name: &'static str,
    /// Permissions the provider benefits from (shown in Settings).
    pub permissions: &'static [Permission],
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("permission required: {0:?}")]
    NeedsPermission(Permission),
    #[error("unavailable: {0}")]
    Unavailable(String),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

impl ProviderError {
    pub fn other(e: impl std::fmt::Display) -> Self {
        Self::Other(e.to_string())
    }
}

/// Everything a provider can report.
#[derive(Debug, Clone)]
pub enum ProviderEvent {
    Notification(NewNotification),
    Battery(BatteryStatus),
    Devices(Vec<DeviceBattery>),
    Weather(Weather),
    Focus(FocusState),
    Status { provider: &'static str, status: ProviderStatus },
}

/// Cooperative cancellation signal shared by every task.
#[derive(Debug, Clone)]
pub struct Shutdown(watch::Receiver<bool>);

#[derive(Debug)]
pub struct ShutdownTrigger(watch::Sender<bool>);

pub fn shutdown_pair() -> (ShutdownTrigger, Shutdown) {
    let (tx, rx) = watch::channel(false);
    (ShutdownTrigger(tx), Shutdown(rx))
}

impl ShutdownTrigger {
    pub fn trigger(&self) {
        self.0.send_replace(true);
    }
}

impl Shutdown {
    pub fn is_triggered(&self) -> bool {
        *self.0.borrow()
    }

    /// Resolves once shutdown is triggered (or the trigger is dropped).
    pub async fn wait(&mut self) {
        while !*self.0.borrow_and_update() {
            if self.0.changed().await.is_err() {
                return;
            }
        }
    }
}

/// Why [`ProviderContext::sleep_or_refresh`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    Timeout,
    Refresh,
    Shutdown,
}

/// Handles given to a running provider.
pub struct ProviderContext {
    pub id: &'static str,
    pub events: mpsc::Sender<ProviderEvent>,
    pub config: watch::Receiver<Arc<Config>>,
    pub paths: Arc<Paths>,
    /// Fired when the user opens the panel — refresh cheap data now.
    pub refresh: Arc<Notify>,
    /// Latest location from CoreLocation (if permitted).
    pub location: watch::Receiver<Option<Coordinates>>,
    pub shutdown: Shutdown,
}

impl ProviderContext {
    pub fn config(&self) -> Arc<Config> {
        self.config.borrow().clone()
    }

    /// Send an event; returns `false` when the engine is gone (app quitting).
    pub async fn emit(&self, event: ProviderEvent) -> bool {
        self.events.send(event).await.is_ok()
    }

    pub async fn set_status(&self, status: ProviderStatus) {
        let _ = self.events.send(ProviderEvent::Status { provider: self.id, status }).await;
    }

    /// Sleep for `dur`, waking early on a refresh request or shutdown.
    pub async fn sleep_or_refresh(&mut self, dur: Duration) -> Wake {
        if self.shutdown.is_triggered() {
            return Wake::Shutdown;
        }
        let refresh = self.refresh.clone();
        tokio::select! {
            _ = tokio::time::sleep(dur) => Wake::Timeout,
            _ = refresh.notified() => Wake::Refresh,
            _ = self.shutdown.wait() => Wake::Shutdown,
        }
    }

    /// Sleep for `dur`, waking early only on shutdown.
    pub async fn sleep(&mut self, dur: Duration) -> Wake {
        if self.shutdown.is_triggered() {
            return Wake::Shutdown;
        }
        tokio::select! {
            _ = tokio::time::sleep(dur) => Wake::Timeout,
            _ = self.shutdown.wait() => Wake::Shutdown,
        }
    }
}

pub trait Provider: Send + 'static {
    fn descriptor(&self) -> ProviderDescriptor;

    /// Run until shutdown. Returning `Err` makes the supervisor restart the
    /// provider with backoff; returning `Ok(())` before shutdown parks it until
    /// the config changes.
    fn run(self: Box<Self>, ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>>;
}

/// Registration entry for the [`Supervisor`].
pub struct ProviderSpec {
    pub descriptor: ProviderDescriptor,
    /// Is the provider enabled in this config?
    pub enabled: fn(&Config) -> bool,
    /// The provider is restarted whenever this value changes (e.g. the HTTP
    /// port). Return an empty string if live config reads are enough.
    pub restart_key: fn(&Config) -> String,
    pub make: Box<dyn Fn() -> Box<dyn Provider> + Send + Sync>,
}

/// Shared channels handed to every provider.
#[derive(Clone)]
pub struct ProviderChannels {
    pub events: mpsc::Sender<ProviderEvent>,
    pub config: watch::Receiver<Arc<Config>>,
    pub paths: Arc<Paths>,
    pub refresh: Arc<Notify>,
    pub location: watch::Receiver<Option<Coordinates>>,
    pub shutdown: Shutdown,
}

/// Starts providers, restarts failed ones and follows config changes.
pub struct Supervisor;

const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
const PERMISSION_RETRY: Duration = Duration::from_secs(30);

impl Supervisor {
    /// Spawn one supervising task per spec on the current tokio runtime.
    pub fn spawn_all(specs: Vec<ProviderSpec>, ch: ProviderChannels) -> Vec<tokio::task::JoinHandle<()>> {
        specs.into_iter().map(|spec| tokio::spawn(Self::supervise(spec, ch.clone()))).collect()
    }

    async fn supervise(spec: ProviderSpec, ch: ProviderChannels) {
        let id = spec.descriptor.id;
        let mut config = ch.config.clone();
        let mut shutdown = ch.shutdown.clone();
        let mut backoff = MIN_BACKOFF;
        let status = |s: ProviderStatus| {
            let events = ch.events.clone();
            async move {
                let _ = events.send(ProviderEvent::Status { provider: id, status: s }).await;
            }
        };

        loop {
            if shutdown.is_triggered() {
                break;
            }
            let cfg = config.borrow_and_update().clone();
            if !(spec.enabled)(&cfg) {
                status(ProviderStatus::Disabled).await;
                tokio::select! {
                    r = config.changed() => if r.is_err() { break },
                    _ = shutdown.wait() => break,
                }
                continue;
            }

            let key = (spec.restart_key)(&cfg);
            status(ProviderStatus::Starting).await;
            let ctx = ProviderContext {
                id,
                events: ch.events.clone(),
                config: ch.config.clone(),
                paths: ch.paths.clone(),
                refresh: ch.refresh.clone(),
                location: ch.location.clone(),
                shutdown: ch.shutdown.clone(),
            };
            let fut = (spec.make)().run(ctx);
            tokio::pin!(fut);

            // Run until it finishes, shutdown, or a relevant config change.
            let outcome = loop {
                tokio::select! {
                    r = &mut fut => break Some(r),
                    _ = shutdown.wait() => break None,
                    r = config.changed() => {
                        if r.is_err() { break None; }
                        let new = config.borrow_and_update().clone();
                        if !(spec.enabled)(&new) || (spec.restart_key)(&new) != key {
                            tracing::info!(provider = id, "restarting after config change");
                            break Some(Ok(()));
                        }
                    }
                }
            };

            match outcome {
                None => break,
                Some(Ok(())) => {
                    backoff = MIN_BACKOFF;
                    // Restart only if the config changed; otherwise park until it does.
                    let now = config.borrow().clone();
                    if (spec.enabled)(&now) && (spec.restart_key)(&now) == key {
                        status(ProviderStatus::Stopped).await;
                        tokio::select! {
                            r = config.changed() => if r.is_err() { break },
                            _ = shutdown.wait() => break,
                        }
                    }
                }
                Some(Err(ProviderError::NeedsPermission(p))) => {
                    tracing::info!(provider = id, permission = ?p, "waiting for permission");
                    status(ProviderStatus::NeedsPermission(p)).await;
                    tokio::select! {
                        _ = tokio::time::sleep(PERMISSION_RETRY) => {},
                        _ = ch.refresh.notified() => {},
                        r = config.changed() => if r.is_err() { break },
                        _ = shutdown.wait() => break,
                    }
                }
                Some(Err(e)) => {
                    tracing::warn!(provider = id, error = %e, retry_in = ?backoff, "provider failed");
                    status(ProviderStatus::Failing(e.to_string())).await;
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {},
                        r = config.changed() => if r.is_err() { break },
                        _ = shutdown.wait() => break,
                    }
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
        status(ProviderStatus::Stopped).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Flaky(Arc<AtomicU32>);

    impl Provider for Flaky {
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor { id: "flaky", name: "Flaky", permissions: &[] }
        }
        fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
            Box::pin(async move {
                let n = self.0.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    return Err(ProviderError::Other(format!("boom {n}")));
                }
                ctx.set_status(ProviderStatus::Running).await;
                ctx.shutdown.wait().await;
                Ok(())
            })
        }
    }

    fn channels() -> (ProviderChannels, mpsc::Receiver<ProviderEvent>, ShutdownTrigger, watch::Sender<Arc<Config>>) {
        let (tx, rx) = mpsc::channel(64);
        let (cfg_tx, cfg_rx) = watch::channel(Arc::new(Config::default()));
        let (_loc_tx, loc_rx) = watch::channel(None);
        let (trigger, shutdown) = shutdown_pair();
        let ch = ProviderChannels {
            events: tx,
            config: cfg_rx,
            paths: Arc::new(Paths::rooted(&std::env::temp_dir())),
            refresh: Arc::new(Notify::new()),
            location: loc_rx,
            shutdown,
        };
        std::mem::forget(_loc_tx);
        (ch, rx, trigger, cfg_tx)
    }

    #[tokio::test(start_paused = true)]
    async fn restarts_with_backoff_then_runs() {
        let (ch, mut rx, trigger, _cfg) = channels();
        let runs = Arc::new(AtomicU32::new(0));
        let r = runs.clone();
        let spec = ProviderSpec {
            descriptor: ProviderDescriptor { id: "flaky", name: "Flaky", permissions: &[] },
            enabled: |_| true,
            restart_key: |_| String::new(),
            make: Box::new(move || Box::new(Flaky(r.clone()))),
        };
        let handles = Supervisor::spawn_all(vec![spec], ch);
        let mut saw_failing = 0;
        loop {
            match rx.recv().await {
                Some(ProviderEvent::Status { status: ProviderStatus::Failing(_), .. }) => saw_failing += 1,
                Some(ProviderEvent::Status { status: ProviderStatus::Running, .. }) => break,
                Some(_) => {}
                None => panic!("channel closed"),
            }
        }
        assert_eq!(saw_failing, 2);
        assert_eq!(runs.load(Ordering::SeqCst), 3);
        trigger.trigger();
        for h in handles {
            h.await.expect("join");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn disabled_provider_starts_when_enabled() {
        let (ch, mut rx, trigger, cfg_tx) = channels();
        let runs = Arc::new(AtomicU32::new(2)); // skip the failures
        let r = runs.clone();
        let spec = ProviderSpec {
            descriptor: ProviderDescriptor { id: "flaky", name: "Flaky", permissions: &[] },
            enabled: |c| c.providers.focus,
            restart_key: |_| String::new(),
            make: Box::new(move || Box::new(Flaky(r.clone()))),
        };
        let mut cfg = Config::default();
        cfg.providers.focus = false;
        cfg_tx.send_replace(Arc::new(cfg.clone()));
        let _h = Supervisor::spawn_all(vec![spec], ch);
        loop {
            if let Some(ProviderEvent::Status { status: ProviderStatus::Disabled, .. }) = rx.recv().await {
                break;
            }
        }
        cfg.providers.focus = true;
        cfg_tx.send_replace(Arc::new(cfg));
        loop {
            if let Some(ProviderEvent::Status { status: ProviderStatus::Running, .. }) = rx.recv().await {
                break;
            }
        }
        trigger.trigger();
    }
}
