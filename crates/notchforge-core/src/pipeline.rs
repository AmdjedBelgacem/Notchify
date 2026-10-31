//! Notification pipeline: filter → dedupe → rate-limit → group → rank.
//!
//! The pipeline is the reason the notch never becomes noisy:
//!
//! * **Filter** drops disabled apps, muted keywords and — while a Focus mode is
//!   active — anything below `focus_min_priority`.
//! * **Dedupe** drops a notification whose normalised (app, title, body) was
//!   seen within `dedupe_window_secs`. The Notification Center DB watcher and
//!   the Accessibility observer can both see the same banner; this collapses
//!   them.
//! * **Rate limit** gives each app a token bucket. When it is empty, new
//!   notifications are *folded into* the app's latest entry rather than dropped,
//!   so nothing is lost but the UI does not churn.
//! * **Group** folds notifications from the same app + thread within
//!   `group_window_secs` into one entry with a `group_count`.
//! * **Rank** orders by priority, then recency.

use crate::apps::AppCategory;
use crate::config::NotificationsConfig;
use crate::model::{NewNotification, Notification, NotificationId, Priority};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Why a notification was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterReason {
    Paused,
    AppDisabled,
    UnknownAppDisallowed,
    MutedKeyword(String),
    FocusActive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A new entry was created.
    Accepted {
        id: NotificationId,
        priority: Priority,
    },
    /// Folded into an existing entry (grouping or rate limiting).
    Grouped {
        id: NotificationId,
        rate_limited: bool,
    },
    Duplicate,
    Filtered(FilterReason),
}

/// Runtime flags the pipeline needs besides the config.
#[derive(Debug, Clone, Copy, Default)]
pub struct PipelineFlags {
    pub paused: bool,
    pub focus_active: bool,
}

#[derive(Debug, Clone, Copy)]
struct TokenBucket {
    tokens: f64,
    last_ms: i64,
}

impl TokenBucket {
    fn take(&mut self, now_ms: i64, burst: u32, refill_secs: u64) -> bool {
        let elapsed = (now_ms - self.last_ms).max(0) as f64 / 1000.0;
        self.tokens = (self.tokens + elapsed / refill_secs as f64).min(burst as f64);
        self.last_ms = now_ms;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Default)]
pub struct Pipeline {
    next_id: u64,
    items: Vec<Notification>,
    seen: HashMap<u64, i64>,
    buckets: HashMap<String, TokenBucket>,
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn dedupe_key(n: &NewNotification) -> u64 {
    let mut h = DefaultHasher::new();
    n.app.rule_key().hash(&mut h);
    normalize(&n.title).hash(&mut h);
    normalize(&n.body).hash(&mut h);
    h.finish()
}

impl Pipeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run a notification through every stage.
    pub fn ingest(
        &mut self,
        n: NewNotification,
        cfg: &NotificationsConfig,
        flags: PipelineFlags,
        now_ms: i64,
    ) -> Outcome {
        // ── filter ──
        if flags.paused {
            return Outcome::Filtered(FilterReason::Paused);
        }
        let key = n.app.rule_key();
        let rule = cfg.rule(&key);
        if n.app.known.is_none() && rule.is_none() && !cfg.allow_other_apps {
            return Outcome::Filtered(FilterReason::UnknownAppDisallowed);
        }
        if let Some(rule) = rule {
            if !rule.enabled {
                return Outcome::Filtered(FilterReason::AppDisabled);
            }
            let haystack = format!("{} {} {}", n.title, n.subtitle.as_deref().unwrap_or(""), n.body).to_lowercase();
            if let Some(word) =
                rule.muted_keywords.iter().find(|w| !w.trim().is_empty() && haystack.contains(&w.to_lowercase()))
            {
                return Outcome::Filtered(FilterReason::MutedKeyword(word.clone()));
            }
        }
        let priority = rule.and_then(|r| r.priority).or(n.priority).unwrap_or_else(|| match n.app.category() {
            AppCategory::Agent => n.kind.default_priority().max(Priority::Normal),
            _ => Priority::Normal,
        });
        if flags.focus_active && priority < cfg.focus_min_priority {
            return Outcome::Filtered(FilterReason::FocusActive);
        }

        // ── dedupe ──
        let window_ms = (cfg.dedupe_window_secs * 1000) as i64;
        self.seen.retain(|_, t| now_ms - *t <= window_ms);
        let dk = dedupe_key(&n);
        if self.seen.contains_key(&dk) {
            return Outcome::Duplicate;
        }
        self.seen.insert(dk, now_ms);

        // ── group ──
        let group_ms = (cfg.group_window_secs * 1000) as i64;
        let same_thread = |item: &Notification| {
            item.app.rule_key() == key
                && !item.read
                && now_ms - item.timestamp_ms <= group_ms
                && match (&item.thread, &n.thread) {
                    (Some(a), Some(b)) => a == b,
                    (None, None) => item.title == n.title,
                    _ => false,
                }
        };
        if let Some(idx) = self.items.iter().rposition(same_thread) {
            let id = self.fold(idx, n, priority);
            return Outcome::Grouped { id, rate_limited: false };
        }

        // ── rate limit ──
        let bucket = self
            .buckets
            .entry(key.clone())
            .or_insert(TokenBucket { tokens: cfg.rate_limit_burst as f64, last_ms: now_ms });
        if !bucket.take(now_ms, cfg.rate_limit_burst, cfg.rate_limit_refill_secs) {
            if let Some(idx) = self.items.iter().rposition(|i| i.app.rule_key() == key) {
                let id = self.fold(idx, n, priority);
                return Outcome::Grouped { id, rate_limited: true };
            }
        }

        // ── accept ──
        self.next_id += 1;
        let id = NotificationId(self.next_id);
        self.items.push(Notification {
            id,
            app: n.app,
            origin: n.origin,
            kind: n.kind,
            priority,
            title: n.title,
            subtitle: n.subtitle,
            body: n.body,
            timestamp_ms: n.timestamp_ms,
            thread: n.thread,
            url: n.url,
            group_count: 1,
            read: false,
        });
        self.enforce_capacity(cfg.max_items);
        Outcome::Accepted { id, priority }
    }

    fn fold(&mut self, idx: usize, n: NewNotification, priority: Priority) -> NotificationId {
        let item = &mut self.items[idx];
        item.group_count += 1;
        item.priority = item.priority.max(priority);
        item.kind = n.kind;
        item.title = n.title;
        item.subtitle = n.subtitle;
        item.body = n.body;
        item.timestamp_ms = item.timestamp_ms.max(n.timestamp_ms);
        if n.url.is_some() {
            item.url = n.url;
        }
        item.id
    }

    /// Drop the least important, oldest entries beyond `max`.
    fn enforce_capacity(&mut self, max: usize) {
        while self.items.len() > max {
            let victim =
                self.items.iter().enumerate().min_by_key(|(_, i)| (i.priority, i.timestamp_ms)).map(|(idx, _)| idx);
            match victim {
                Some(idx) => {
                    self.items.remove(idx);
                }
                None => break,
            }
        }
    }

    /// Entries ranked by priority (desc) then recency (desc).
    pub fn ranked(&self) -> Vec<Notification> {
        let mut v = self.items.clone();
        v.sort_by(|a, b| b.priority.cmp(&a.priority).then(b.timestamp_ms.cmp(&a.timestamp_ms)));
        v
    }

    /// Most recent entry, regardless of priority (what a peek shows).
    pub fn latest(&self) -> Option<&Notification> {
        self.items.iter().max_by_key(|i| (i.timestamp_ms, i.id))
    }

    pub fn get(&self, id: NotificationId) -> Option<&Notification> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn dismiss(&mut self, id: NotificationId) -> bool {
        let before = self.items.len();
        self.items.retain(|i| i.id != id);
        before != self.items.len()
    }

    pub fn dismiss_app(&mut self, rule_key: &str) {
        self.items.retain(|i| i.app.rule_key() != rule_key);
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn mark_all_read(&mut self) -> bool {
        let mut changed = false;
        for i in self.items.iter_mut().filter(|i| !i.read) {
            i.read = true;
            changed = true;
        }
        changed
    }

    pub fn unread_count(&self) -> usize {
        self.items.iter().filter(|i| !i.read).count()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::{AppRef, KnownApp};
    use crate::config::AppRule;
    use crate::model::{NotificationKind, Origin};

    fn note(app: KnownApp, title: &str, body: &str) -> NewNotification {
        NewNotification {
            app: AppRef::known(app),
            origin: Origin::AgentWebhook,
            kind: NotificationKind::Message,
            priority: None,
            title: title.into(),
            subtitle: None,
            body: body.into(),
            timestamp_ms: 0,
            thread: None,
            url: None,
        }
    }

    fn cfg() -> NotificationsConfig {
        NotificationsConfig::default()
    }

    #[test]
    fn accepts_and_ranks() {
        let mut p = Pipeline::new();
        let c = cfg();
        let f = PipelineFlags::default();
        let mut low = note(KnownApp::Slack, "a", "1");
        low.priority = Some(Priority::Low);
        let mut high = note(KnownApp::Claude, "b", "2");
        high.kind = NotificationKind::NeedsInput;
        assert!(matches!(p.ingest(low, &c, f, 1000), Outcome::Accepted { .. }));
        assert!(matches!(p.ingest(high, &c, f, 2000), Outcome::Accepted { priority: Priority::High, .. }));
        let ranked = p.ranked();
        assert_eq!(ranked[0].title, "b");
        assert_eq!(p.unread_count(), 2);
    }

    #[test]
    fn dedupes_within_window_only() {
        let mut p = Pipeline::new();
        let c = cfg();
        let f = PipelineFlags::default();
        assert!(matches!(p.ingest(note(KnownApp::Discord, "Hi", "there"), &c, f, 0), Outcome::Accepted { .. }));
        // Whitespace/case differences (AX vs DB text) still dedupe.
        assert_eq!(p.ingest(note(KnownApp::Discord, "hi ", "There"), &c, f, 5_000), Outcome::Duplicate);
        // After the window, the same text is a new event (grouped with the first).
        assert!(matches!(p.ingest(note(KnownApp::Discord, "Hi", "there"), &c, f, 20_000), Outcome::Grouped { .. }));
    }

    #[test]
    fn filters_disabled_apps_keywords_pause_and_focus() {
        let mut p = Pipeline::new();
        let mut c = cfg();
        c.apps.insert("discord".into(), AppRule { enabled: false, ..AppRule::default() });
        c.apps.insert("slack".into(), AppRule { muted_keywords: vec!["standup".into()], ..AppRule::default() });
        let f = PipelineFlags::default();
        assert_eq!(p.ingest(note(KnownApp::Discord, "x", "y"), &c, f, 0), Outcome::Filtered(FilterReason::AppDisabled));
        assert_eq!(
            p.ingest(note(KnownApp::Slack, "Daily StandUp", ""), &c, f, 0),
            Outcome::Filtered(FilterReason::MutedKeyword("standup".into()))
        );
        let paused = PipelineFlags { paused: true, ..f };
        assert_eq!(p.ingest(note(KnownApp::Slack, "a", "b"), &c, paused, 0), Outcome::Filtered(FilterReason::Paused));
        let focus = PipelineFlags { focus_active: true, ..f };
        assert_eq!(
            p.ingest(note(KnownApp::Slack, "a", "b"), &c, focus, 0),
            Outcome::Filtered(FilterReason::FocusActive)
        );
        let mut urgent = note(KnownApp::Slack, "prod down", "");
        urgent.priority = Some(Priority::Critical);
        assert!(matches!(p.ingest(urgent, &c, focus, 0), Outcome::Accepted { .. }));
    }

    #[test]
    fn unknown_apps_can_be_disallowed() {
        let mut p = Pipeline::new();
        let mut c = cfg();
        c.allow_other_apps = false;
        let mut n = note(KnownApp::Slack, "a", "b");
        n.app = AppRef::resolve("com.example.thing", "", &[]);
        assert_eq!(p.ingest(n, &c, PipelineFlags::default(), 0), Outcome::Filtered(FilterReason::UnknownAppDisallowed));
    }

    #[test]
    fn groups_by_thread() {
        let mut p = Pipeline::new();
        let c = cfg();
        let f = PipelineFlags::default();
        let mut a = note(KnownApp::WhatsApp, "Alice", "hey");
        a.thread = Some("alice".into());
        let mut b = note(KnownApp::WhatsApp, "Alice", "you there?");
        b.thread = Some("alice".into());
        b.timestamp_ms = 1000;
        let Outcome::Accepted { id, .. } = p.ingest(a, &c, f, 0) else { panic!("not accepted") };
        assert_eq!(p.ingest(b, &c, f, 1000), Outcome::Grouped { id, rate_limited: false });
        let item = p.get(id).expect("exists");
        assert_eq!(item.group_count, 2);
        assert_eq!(item.body, "you there?");
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn rate_limit_folds_bursts() {
        let mut p = Pipeline::new();
        let mut c = cfg();
        c.rate_limit_burst = 3;
        let f = PipelineFlags::default();
        let mut accepted = 0;
        let mut limited = 0;
        for i in 0..10 {
            match p.ingest(note(KnownApp::Discord, &format!("t{i}"), "b"), &c, f, 10) {
                Outcome::Accepted { .. } => accepted += 1,
                Outcome::Grouped { rate_limited: true, .. } => limited += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(accepted, 3);
        assert_eq!(limited, 7);
        // Tokens refill over time.
        assert!(matches!(p.ingest(note(KnownApp::Discord, "later", "b"), &c, f, 10 + 3_000), Outcome::Accepted { .. }));
    }

    #[test]
    fn capacity_evicts_least_important() {
        let mut p = Pipeline::new();
        let mut c = cfg();
        c.max_items = 10;
        c.rate_limit_burst = 1000;
        let f = PipelineFlags::default();
        let mut vip = note(KnownApp::Claude, "vip", "");
        vip.priority = Some(Priority::Critical);
        p.ingest(vip, &c, f, 0);
        for i in 0..20 {
            p.ingest(note(KnownApp::Slack, &format!("n{i}"), ""), &c, f, i + 1);
        }
        assert_eq!(p.len(), 10);
        assert!(p.ranked().iter().any(|n| n.title == "vip"));
    }

    #[test]
    fn dismiss_clear_and_read() {
        let mut p = Pipeline::new();
        let c = cfg();
        let f = PipelineFlags::default();
        let Outcome::Accepted { id, .. } = p.ingest(note(KnownApp::Zed, "a", ""), &c, f, 0) else { panic!() };
        p.ingest(note(KnownApp::Zed, "b", ""), &c, f, 1);
        assert!(p.mark_all_read());
        assert_eq!(p.unread_count(), 0);
        assert!(!p.mark_all_read());
        assert!(p.dismiss(id));
        assert!(!p.dismiss(id));
        p.clear();
        assert!(p.is_empty());
    }
}
