//! Calendar events and accounts, read through EventKit (public API).
//!
//! One provider owns everything: it requests Calendar access (macOS shows the
//! system prompt), then reads the connected accounts ("calendar connections" —
//! iCloud, Google, Exchange, subscribed, local) and the upcoming events, and
//! emits them as a [`CalendarState`] the Overview and Planner render. Events are
//! read-only; reminders can be added and completed from the Planner.

use notchforge_core::model::Permission::Calendar as CalendarPermission;
#[cfg(target_os = "macos")]
use notchforge_core::model::{CalendarEvent, PermissionState, ReminderItem};
use notchforge_core::model::{CalendarState, ProviderStatus};
#[cfg(target_os = "macos")]
use notchforge_core::provider::Wake;
use notchforge_core::provider::{
    BoxFuture, Provider, ProviderCommand, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent,
};
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "macos")]
pub use mac::{authorization_state, request_access};

/// Complete a reminder by id, on the main thread (UI-facing helper).
#[cfg(target_os = "macos")]
pub fn complete_reminder_by_id(id: &str) -> bool {
    let id = id.to_owned();
    on_main(move |mtm| {
        let store = mac::store_ref(mtm);
        mac::complete_reminder(&store, &id)
    })
    .unwrap_or(false)
}

/// Add a reminder to the default Reminders list, on the main thread.
#[cfg(target_os = "macos")]
pub fn add_reminder(title: &str) -> bool {
    add_reminder_due(title, None)
}

/// Add a reminder, optionally due at an absolute epoch-ms time.
#[cfg(target_os = "macos")]
pub fn add_reminder_due(title: &str, due_ms: Option<i64>) -> bool {
    let title = title.trim().to_owned();
    if title.is_empty() {
        return false;
    }
    on_main(move |mtm| {
        let store = mac::store_ref(mtm);
        mac::add_reminder(&store, &title, due_ms)
    })
    .unwrap_or(false)
}

/// Local midnight today, and today's place in the week (Monday is 0).
pub fn local_day(now_ms: i64) -> (i64, u8) {
    // SAFETY: localtime_r and mktime work on caller-owned structs.
    unsafe {
        let t = (now_ms / 1000) as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let weekday = ((tm.tm_wday + 6) % 7) as u8;
        tm.tm_hour = 0;
        tm.tm_min = 0;
        tm.tm_sec = 0;
        tm.tm_isdst = -1;
        (libc::mktime(&mut tm) as i64 * 1000, weekday)
    }
}

/// The day of the month at `ms`, in local time.
#[cfg(target_os = "macos")]
fn day_of_month(ms: i64) -> u8 {
    // SAFETY: as above.
    unsafe {
        let t = (ms / 1000) as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm.tm_mday as u8
    }
}

pub struct CalendarProvider;

/// Poll while things are calm — Calendar.app edits surface within one cycle.
#[cfg(target_os = "macos")]
const POLL: Duration = Duration::from_secs(60);
/// How far ahead the tile lists events.
#[cfg(target_os = "macos")]
const HORIZON_MS: i64 = 12 * 3600 * 1000;
#[cfg(target_os = "macos")]
const MAX_EVENTS: usize = 6;

#[cfg(target_os = "macos")]
mod mac {
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
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
                EKAuthorizationStatus::Denied | EKAuthorizationStatus::Restricted => PermissionState::Denied,
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
        let rgb = unsafe { cal.color().colorUsingColorSpace(&objc2_app_kit::NSColorSpace::deviceRGBColorSpace()) }?;
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
                let name = if title.is_empty() { kind.to_owned() } else { format!("{title} ({kind})") };
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
            "zoom.us",
            "zoom.com",
            "meet.google.com",
            "teams.microsoft.com",
            "teams.live.com",
            "webex.com",
            "whereby.com",
            "jitsi",
            "around.co",
            "pop.com",
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
            // Every incomplete reminder, with or without a due date.
            let _ = now_ms;
            let predicate = store.predicateForIncompleteRemindersWithDueDateStarting_ending_calendars(None, None, None);
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
                        let due_at_ms = r
                            .dueDateComponents()
                            .and_then(|dc| objc2_foundation::NSCalendar::currentCalendar().dateFromComponents(&dc))
                            .map(|d| (d.timeIntervalSince1970() * 1000.0).round() as i64);
                        items.push(ReminderItem { id, title, due_at_ms });
                    }
                }
                // Soonest due first; those without a date after them.
                items.sort_by_key(|r| r.due_at_ms.unwrap_or(i64::MAX));
                items.truncate(8);
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

    /// Add a reminder titled `title` to the default list, optionally due at
    /// an absolute time (epoch ms).
    pub fn add_reminder(store: &EKEventStore, title: &str, due_ms: Option<i64>) -> bool {
        // SAFETY: create + save on the main thread.
        unsafe {
            let Some(list) = store.defaultCalendarForNewReminders() else { return false };
            let reminder = objc2_event_kit::EKReminder::reminderWithEventStore(store);
            reminder.setTitle(Some(&NSString::from_str(title)));
            reminder.setCalendar(Some(&list));
            if let Some(ms) = due_ms {
                let date = NSDate::dateWithTimeIntervalSince1970(ms as f64 / 1000.0);
                let comps = objc2_foundation::NSCalendar::currentCalendar().components_fromDate(
                    objc2_foundation::NSCalendarUnit::Year
                        | objc2_foundation::NSCalendarUnit::Month
                        | objc2_foundation::NSCalendarUnit::Day
                        | objc2_foundation::NSCalendarUnit::Hour
                        | objc2_foundation::NSCalendarUnit::Minute,
                    &date,
                );
                reminder.setDueDateComponents(Some(&comps));
            }
            store.saveReminder_commit_error(&reminder, true).is_ok()
        }
    }

    /// Today's events, and which days of this week have one.
    pub fn week(store: &EKEventStore, now_ms: i64) -> (Vec<CalendarEvent>, Vec<bool>, u8, Vec<u8>) {
        const DAY: i64 = 86_400_000;
        let (today, weekday) = super::local_day(now_ms);
        let monday = today - weekday as i64 * DAY;
        let span = |from: i64, to: i64| {
            // SAFETY: read-only EventKit query.
            unsafe {
                let start = NSDate::dateWithTimeIntervalSince1970(from as f64 / 1000.0);
                let end = NSDate::dateWithTimeIntervalSince1970(to as f64 / 1000.0);
                store
                    .eventsMatchingPredicate(
                        &store.predicateForEventsWithStartDate_endDate_calendars(&start, &end, None),
                    )
                    .into_iter()
                    .map(|e| ((e.startDate().timeIntervalSince1970() * 1000.0).round() as i64, e))
                    .collect::<Vec<_>>()
            }
        };
        let in_week = span(monday, monday + 7 * DAY);
        let busy = (0..7)
            .map(|d| {
                let (from, to) = (monday + d * DAY, monday + (d + 1) * DAY);
                in_week.iter().any(|(start, _)| (from..to).contains(start))
            })
            .collect();
        let dates = (0..7).map(|d| super::day_of_month(monday + d * DAY + DAY / 2)).collect();
        // SAFETY: read-only accessors on the events just fetched.
        let mut list: Vec<CalendarEvent> = unsafe {
            span(today, today + DAY)
                .into_iter()
                .map(|(starts_at_ms, e)| {
                    let location = e.location().map(|l| l.to_string());
                    let notes = e.notes().map(|n| n.to_string()).unwrap_or_default();
                    let (calendar, color) = match e.calendar() {
                        Some(c) => (c.title().to_string(), color(&c)),
                        None => (String::new(), None),
                    };
                    CalendarEvent {
                        title: e.title().to_string(),
                        starts_at_ms,
                        ends_at_ms: (e.endDate().timeIntervalSince1970() * 1000.0).round() as i64,
                        all_day: e.isAllDay(),
                        calendar,
                        color,
                        meeting_url: meeting_url_in(&location.clone().unwrap_or_default())
                            .or_else(|| meeting_url_in(&notes)),
                        location,
                        url: None,
                    }
                })
                .collect()
        };
        list.sort_by_key(|e| (!e.all_day, e.starts_at_ms));
        list.truncate(8);
        (list, busy, weekday, dates)
    }

    /// Upcoming events within the horizon, soonest first.
    pub fn events(store: &EKEventStore, now_ms: i64) -> Vec<CalendarEvent> {
        let now = now_ms as f64 / 1000.0;
        let start = NSDate::dateWithTimeIntervalSince1970(now - 300.0);
        let end = NSDate::dateWithTimeIntervalSince1970(now + HORIZON_MS as f64 / 1000.0);
        // SAFETY: read-only EventKit queries.
        unsafe {
            let found = store
                .eventsMatchingPredicate(&store.predicateForEventsWithStartDate_endDate_calendars(&start, &end, None));
            let mut list: Vec<CalendarEvent> = found
                .into_iter()
                .map(|e| {
                    let title = e.title().to_string();
                    let starts_at_ms = (e.startDate().timeIntervalSince1970() * 1000.0).round() as i64;
                    let ends_at_ms = (e.endDate().timeIntervalSince1970() * 1000.0).round() as i64;
                    let location = e.location().map(|l| l.to_string());
                    let notes = e.notes().map(|n| n.to_string()).unwrap_or_default();
                    let meeting_url =
                        meeting_url_in(&location.clone().unwrap_or_default()).or_else(|| meeting_url_in(&notes));
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
            let (today, busy_days, weekday, week_dates) = week(store, now_ms);
            CalendarState {
                accounts: accounts(store),
                events: events(store, now_ms),
                reminders,
                granted: true,
                today,
                busy_days,
                weekday,
                week_dates,
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

/// Run `f` on the main thread.
#[cfg(target_os = "macos")]
pub fn on_main<T: Send + 'static>(f: impl FnOnce(objc2::MainThreadMarker) -> T + Send + 'static) -> Option<T> {
    mac::on_main(f)
}

/// Elsewhere there is no EventKit: the provider says so and waits.
#[cfg(not(target_os = "macos"))]
impl Provider for CalendarProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "calendar", name: "Calendar", permissions: &[CalendarPermission] }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            ctx.set_status(ProviderStatus::Failing("Calendar is only available on macOS".into())).await;
            let _ = ctx.emit(ProviderEvent::Calendar(CalendarState::default())).await;
            let _ = ctx.sleep(std::time::Duration::from_secs(u64::MAX / 4)).await;
            Ok(())
        })
    }
}

#[cfg(target_os = "macos")]
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
                    let state =
                        tokio::task::spawn_blocking(mac::authorization_state).await.unwrap_or(PermissionState::Unknown);
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
                let state = on_main(move |mtm| mac::read_state(mtm, now)).unwrap_or_default();
                if last.as_ref() != Some(&state) {
                    let _ = ctx.emit(ProviderEvent::Calendar(state.clone())).await;
                    last = Some(state);
                }
                // Reminder requests from the CLI arrive as commands; save on
                // the main thread where EventKit lives, then refresh at once.
                let cmd = tokio::select! {
                    c = ctx.next_command() => c,
                    _ = tokio::time::sleep(POLL) => None,
                };
                if let Some(ProviderCommand::AddReminder { title, due_ms }) = cmd {
                    let log_title = title.clone();
                    let saved = on_main(move |mtm| {
                        let store = mac::store_ref(mtm);
                        mac::add_reminder(&store, &title, due_ms)
                    })
                    .unwrap_or(false);
                    tracing::info!(saved, title = %log_title, "reminder added from the CLI");
                    last = None; // force a re-read so the list shows it
                    continue;
                } else if cmd.is_none() {
                    return Ok(());
                }
                if ctx.sleep(POLL).await == Wake::Shutdown {
                    return Ok(());
                }
            }
        })
    }
}
