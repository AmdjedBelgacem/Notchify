//! Calendar events and accounts, read through EventKit (public API).
//!
//! One provider owns everything: it requests Calendar access (macOS shows the
//! system prompt), then reads the connected accounts ("calendar connections" —
//! iCloud, Google, Exchange, subscribed, local) and the upcoming events, and
//! emits them as a [`CalendarState`] the Overview card renders. Read-only:
//! nothing in the notch ever writes to a calendar.

use notchforge_core::model::Permission::Calendar as CalendarPermission;
use notchforge_core::model::{
    CalendarEvent, CalendarState, PermissionState, ProviderStatus, ReminderItem,
};
use notchforge_core::provider::{
    BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent, Wake,
};
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "macos")]
pub use mac::{authorization_state, request_access};

/// Complete a reminder by id, on the main thread (UI-facing helper).
pub fn complete_reminder_by_id(id: &str) -> bool {
    let id = id.to_owned();
    on_main(move |mtm| {
        let store = mac::store_ref(mtm);
        mac::complete_reminder(&store, &id)
    })
    .unwrap_or(false)
}

pub struct CalendarProvider;

/// Poll while things are calm — Calendar.app edits surface within one cycle.
#[cfg(target_os = "macos")]
const POLL: Duration = Duration::from_secs(300);
/// How far ahead the tile lists events.
const HORIZON_MS: i64 = 12 * 3600 * 1000;
const MAX_EVENTS: usize = 6;

#[cfg(target_os = "macos")]
mod mac {
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_event_kit::{EKAuthorizationStatus, EKEntityType, EKEventStore, EKSourceType};
    use objc2_foundation::{NSDate, NSError, NSString};
    use std::cell::RefCell;

    use super::*;

    // The store lives on the main thread (EKEventStore is main-thread-only in
    // the bindings) and is reached through `exec_sync` hops.
    thread_local! {
        static STORE: RefCell<Option<Retained<EKEventStore>>> = const { RefCell::new(None) };
    }

    pub fn authorization_state() -> PermissionState {
        // SAFETY: a plain status query with no side effects.
        unsafe {
            match EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) {
                EKAuthorizationStatus::FullAccess => PermissionState::Granted,
                EKAuthorizationStatus::Denied | EKAuthorizationStatus::Restricted => {
                    PermissionState::Denied
                }
                _ => PermissionState::NotDetermined,
            }
        }
    }

    /// Create the store on the main thread if it isn't there yet.
    pub fn ensure_store(mtm: MainThreadMarker) {
        STORE.with(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.is_none() {
                // SAFETY: designated initializer, on the main thread (these
                // closures run via exec_sync on main).
                unsafe {
                    let store = EKEventStore::init(mtm.alloc());
                    *slot = Some(store);
                }
            }
        });
    }

    /// Show the system prompt when access has never been asked for.
    pub fn request_access(mtm: MainThreadMarker) {
        ensure_store(mtm);
        STORE.with(|cell| {
            let slot = cell.borrow();
            let Some(store) = slot.as_ref() else { return };
            // SAFETY: a valid store; the block is retained for the call.
            unsafe {
                let block = RcBlock::new(move |_: objc2::runtime::Bool, _: *mut NSError| {});
                store.requestFullAccessToEventsWithCompletion(RcBlock::as_ptr(&block) as *mut _);
            }
        });
    }

    /// The calendar's colour as RGB, for the dot in the list.
    fn color(cal: &objc2_event_kit::EKCalendar) -> Option<(u8, u8, u8)> {
        // SAFETY: a plain colour-space conversion.
        let rgb = unsafe {
            cal.color().colorUsingColorSpace(&objc2_app_kit::NSColorSpace::deviceRGBColorSpace())
        }?;
        Some((
            (rgb.redComponent() * 255.0).round() as u8,
            (rgb.greenComponent() * 255.0).round() as u8,
            (rgb.blueComponent() * 255.0).round() as u8,
        ))
    }

    /// Connected accounts ("calendar connections"), as human names.
    pub fn accounts(store: &EKEventStore) -> Vec<String> {
        let mut out = Vec::new();
        // SAFETY: reading the AX-free EventKit tree; all calls are plain reads.
        unsafe {
        for source in store.sources() {
            let kind = match source.sourceType() {
                EKSourceType::Subscribed => "Subscribed",
                EKSourceType::CalDAV => "CalDAV",
                EKSourceType::Exchange => "Exchange",
                EKSourceType::MobileMe => "MobileMe",
                EKSourceType::Local => "On this Mac",
                EKSourceType::Birthdays => "Birthdays",
                _ => continue,
            };
            let title = source.title().to_string();
            let name = if title.is_empty() {
                kind.to_owned()
            } else {
                format!("{title} ({kind})")
            };
            if !out.contains(&name) {
                out.push(name);
            }
        }
        out
        }
    }

    /// The video-call link inside an event's text, if any: scans for URLs
    /// hosted by the big meeting services.
    fn meeting_url_in(text: &str) -> Option<String> {
        const HOSTS: &[&str] = &[
            "zoom.us", "zoom.com", "meet.google.com", "teams.microsoft.com", "teams.live.com",
            "webex.com", "whereby.com", "jitsi", "around.co", "pop.com",
        ];
        for candidate in text.split([' ', '\n', '\r', '\t', '"', '<', '>']) {
            if let Some(rest) = candidate.strip_prefix("http://").or_else(|| candidate.strip_prefix("https://")) {
                let host = rest.split('/').next().unwrap_or("").to_lowercase();
                if HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}"))) {
                    return Some(candidate.trim_end_matches(&['.', ',', ';'][..]).to_owned());
                }
            }
        }
        None
    }

    /// Reminders due soon, from the last fetch (the fetch completion runs on a
    /// background queue, so plain data crosses over through this cache).
    static REMINDER_CACHE: std::sync::Mutex<Vec<ReminderItem>> = std::sync::Mutex::new(Vec::new());

    /// Kick an async reminders fetch (incomplete, due from two days ago onward).
    /// Results land in [`REMINDER_CACHE`] and surface on the next read.
    pub fn fetch_reminders(store: &EKEventStore, now_ms: i64) {
        // SAFETY: read-only EventKit query; the completion is retained by
        // EventKit until it fires.
        unsafe {
            let start = NSDate::dateWithTimeIntervalSince1970(now_ms as f64 / 1000.0 - 2.0 * 86400.0);
            let predicate = store.predicateForIncompleteRemindersWithDueDateStarting_ending_calendars(
                Some(&start), None, None,
            );
            let block = RcBlock::new(move |list: *mut objc2_foundation::NSArray<objc2_event_kit::EKReminder>| {
                let mut items: Vec<ReminderItem> = Vec::new();
                if !list.is_null() {
                    let arr = &*list;
                    for r in arr.iter() {
                        if r.isCompleted() {
                            continue;
                        }
                        let id = r.calendarItemExternalIdentifier().map(|s| s.to_string()).unwrap_or_default();
                        let title = r.title().to_string();
                        let mut due = String::new();
                        if let Some(dc) = r.dueDateComponents() {
                            let (h, m) = (dc.hour(), dc.minute());
                            if h >= 0 && m >= 0 {
                                due = format!("{h:02}:{m:02}");
                            }
                        }
                        let title = if due.is_empty() { title } else { format!("{title} ({due})") };
                        items.push(ReminderItem { id, title, due_at_ms: None });
                    }
                }
                items.truncate(4);
                *REMINDER_CACHE.lock().unwrap() = items;
            });
            store.fetchRemindersMatchingPredicate_completion(&predicate, &block);
        }
    }

    /// Mark a reminder complete and save it, looked up fresh by its id.
    pub fn complete_reminder(store: &EKEventStore, id: &str) -> bool {
        // SAFETY: fetch + complete + save, all on the main thread.
        unsafe {
            let id_str = NSString::from_str(id);
            let Some(item) = store.calendarItemWithIdentifier(&id_str) else { return false };
            let Ok(reminder) = item.downcast::<objc2_event_kit::EKReminder>() else { return false };
            reminder.setCompleted(true);
            store.saveReminder_commit_error(&reminder, true).is_ok()
        }
    }

    /// Upcoming events within the horizon, soonest first.
    pub fn events(store: &EKEventStore, now_ms: i64) -> Vec<CalendarEvent> {
        let now = now_ms as f64 / 1000.0;
        let start = NSDate::dateWithTimeIntervalSince1970(now - 300.0);
        let end = NSDate::dateWithTimeIntervalSince1970(now + HORIZON_MS as f64 / 1000.0);
        // SAFETY: read-only EventKit queries.
        unsafe {
        let found = store
            .eventsMatchingPredicate(&store.predicateForEventsWithStartDate_endDate_calendars(
                &start, &end, None,
            ));
        let mut list: Vec<CalendarEvent> = found
            .into_iter()
            .map(|e| {
                let title = e.title().to_string();
                let starts_at_ms = (e.startDate().timeIntervalSince1970() * 1000.0).round() as i64;
                let ends_at_ms = (e.endDate().timeIntervalSince1970() * 1000.0).round() as i64;
                let location = e.location().map(|l| l.to_string());
                let notes = e.notes().map(|n| n.to_string()).unwrap_or_default();
                let meeting_url = meeting_url_in(&location.clone().unwrap_or_default())
                    .or_else(|| meeting_url_in(&notes));
                let (calendar, color) = match e.calendar() {
                    Some(c) => (c.title().to_string(), color(&c)),
                    None => (String::new(), None),
                };
                CalendarEvent {
                    title,
                    starts_at_ms,
                    ends_at_ms,
                    all_day: e.isAllDay(),
                    calendar,
                    color,
                    location,
                    meeting_url,
                    url: None,
                }
            })
            .collect();
        list.sort_by_key(|e| e.starts_at_ms);
        list.retain(|e| e.starts_at_ms >= now_ms - 300_000);
        list.truncate(MAX_EVENTS);
        list
        }
    }

    /// The live store, creating it if needed.
    pub fn store_ref(mtm: MainThreadMarker) -> Retained<EKEventStore> {
        ensure_store(mtm);
        STORE.with_borrow(|slot| {
            slot.clone().unwrap_or_else(|| {
                // SAFETY: on the main thread.
                unsafe { EKEventStore::init(mtm.alloc()) }
            })
        })
    }

    /// Accounts + events + reminders in one main-thread hop.
    pub fn read_state(mtm: MainThreadMarker, now_ms: i64) -> CalendarState {
        ensure_store(mtm);
        STORE.with(|cell| {
            let slot = cell.borrow();
            let Some(store) = slot.as_ref() else {
                return CalendarState::default();
            };
            fetch_reminders(store, now_ms);
            let reminders = REMINDER_CACHE.lock().unwrap().clone();
            CalendarState {
                accounts: accounts(store),
                events: events(store, now_ms),
                reminders,
                granted: true,
            }
        })
    }

    /// Run a closure on the main thread and take its result through a channel.
    pub fn on_main<T: Send + 'static>(f: impl FnOnce(MainThreadMarker) -> T + Send + 'static) -> Option<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        dispatch2::DispatchQueue::main().exec_sync(move || {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let _ = tx.send(f(mtm));
        });
        rx.recv().ok()
    }
}

/// Run `f` on the main thread, or nowhere when off-macOS.
pub fn on_main<T: Send + 'static>(f: impl FnOnce(objc2::MainThreadMarker) -> T + Send + 'static) -> Option<T> {
    mac::on_main(f)
}

impl Provider for CalendarProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "calendar", name: "Calendar", permissions: &[CalendarPermission] }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            if !cfg!(target_os = "macos") {
                ctx.set_status(ProviderStatus::Failing("Calendar is only available on macOS".into())).await;
                ctx.shutdown.wait().await;
                return Ok(());
            }

            // Request access once: macOS shows its prompt if never asked.
            let asked = on_main(|mtm| {
                mac::ensure_store(mtm);
                mac::request_access(mtm);
                mac::authorization_state()
            })
            .unwrap_or(PermissionState::Unknown);

            if asked != PermissionState::Granted {
                ctx.set_status(ProviderStatus::NeedsPermission(CalendarPermission)).await;
                let _ = ctx.emit(ProviderEvent::Calendar(CalendarState::default())).await;
                // Keep watching: the user can grant in Settings at any time.
                loop {
                    let state = tokio::task::spawn_blocking(mac::authorization_state)
                        .await
                        .unwrap_or(PermissionState::Unknown);
                    if state == PermissionState::Granted {
                        break;
                    }
                    if ctx.sleep(POLL).await == Wake::Shutdown {
                        return Ok(());
                    }
                }
            }

            ctx.set_status(ProviderStatus::Running).await;

            let mut last: Option<CalendarState> = None;
            loop {
                let now = notchforge_core::time::now_ms();
                // EKEventStore is main-thread-only: hop over and read there.
                let state = on_main(move |mtm| mac::read_state(mtm, now))
                    .unwrap_or_default();
                if last.as_ref() != Some(&state) {
                    let _ = ctx.emit(ProviderEvent::Calendar(state.clone())).await;
                    last = Some(state);
                }
                if ctx.sleep(POLL).await == Wake::Shutdown {
                    return Ok(());
                }
            }
        })
    }
}
