//! Headless UI sink: logs what the notch would show.

use notchforge_core::engine::{Snapshot, UiSink};
use notchforge_core::model::Notification;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct LogSink {
    /// Highest notification id already logged.
    seen: AtomicU64,
}

impl UiSink for LogSink {
    fn publish(&self, s: Arc<Snapshot>) {
        let seen = self.seen.load(Ordering::Relaxed);
        let mut fresh: Vec<&Notification> = s.notifications.iter().filter(|n| n.id.0 > seen).collect();
        fresh.sort_by_key(|n| n.id);
        for n in &fresh {
            tracing::info!(app = %n.app.display_name, kind = ?n.kind, priority = ?n.priority, title = %n.title, body = %n.body, "notification");
        }
        if let Some(max) = fresh.last() {
            self.seen.store(max.id.0, Ordering::Relaxed);
        }
        tracing::debug!(
            generation = s.generation,
            unread = s.unread,
            tray = s.tray.len(),
            battery = ?s.battery.as_ref().map(|b| b.percent),
            devices = s.devices.len(),
            weather = ?s.weather.as_ref().map(|w| (w.place.clone(), w.temperature)),
            output = ?s.audio.default_device().map(|d| format!("{} ({})", d.name, d.capabilities())),
            outputs = s.audio.devices.len(),
            playing = ?s.audio.sources.iter().map(|a| format!("{} -> {}", a.name, s.audio.route_label(a))).collect::<Vec<_>>(),
            now_playing = ?s.media.primary().map(|n| format!("{:?} {:?}: {} - {}", n.player, n.state, n.title, n.artist)),
            playlists = s.media.playlists.len(),
            playlist_status = ?s.media.playlist_status,
            running_players = ?s.media.running,
            providers = ?s.providers,
            "snapshot"
        );
    }

    fn peek(&self, n: Notification) {
        tracing::debug!(title = %n.title, "would peek");
    }
}
