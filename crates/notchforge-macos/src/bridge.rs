//! Delivery of engine output onto the AppKit main thread.
//!
//! The engine runs on a tokio worker. [`MainThreadSink`] stores the latest
//! snapshot and schedules **at most one** pending main-queue block at a time,
//! so the UI renders only the newest state no matter how fast the engine
//! publishes.

use notchforge_core::config::Config;
use notchforge_core::engine::{Snapshot, UiSink};
use notchforge_core::model::Notification;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Default)]
pub struct MainThreadSink {
    latest: Mutex<Option<Arc<Snapshot>>>,
    scheduled: AtomicBool,
}

static SINK: OnceLock<Arc<MainThreadSink>> = OnceLock::new();

/// The process-wide sink (created on first use).
pub fn sink() -> Arc<MainThreadSink> {
    SINK.get_or_init(|| Arc::new(MainThreadSink::default())).clone()
}

fn on_main(f: impl FnOnce() + Send + 'static) {
    dispatch2::DispatchQueue::main().exec_async(f);
}

impl MainThreadSink {
    fn take_latest(&self) -> Option<Arc<Snapshot>> {
        self.scheduled.store(false, Ordering::Release);
        self.latest.lock().ok()?.take()
    }
}

impl UiSink for MainThreadSink {
    fn publish(&self, snapshot: Arc<Snapshot>) {
        if let Ok(mut l) = self.latest.lock() {
            *l = Some(snapshot);
        }
        if !self.scheduled.swap(true, Ordering::AcqRel) {
            on_main(|| {
                if let Some(snap) = sink().take_latest() {
                    crate::app::apply_snapshot(snap);
                }
            });
        }
    }

    fn peek(&self, notification: Notification) {
        on_main(move || {
            crate::app::with_controller(|c| c.peek(notification));
        });
    }

    fn config_changed(&self, config: Arc<Config>) {
        on_main(move || crate::app::apply_config(config));
    }

    fn native_banner_handled(&self, notification: &Notification) {
        let n = notification.clone();
        on_main(move || crate::accessibility::dismiss_banner(&n));
    }
}
