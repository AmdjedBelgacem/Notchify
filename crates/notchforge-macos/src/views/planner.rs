//! "Planner" page of the expanded panel, built to the *Pinned · Planner* board
//! of the redesign twin: today in the calendar (a 2 × 2 block), plans from
//! Reminders and notes typed into the notch (a column each).
//!
//! ```text
//! ╭ Saturday 7 November ───── 4 events ╮ ╭ Plans  3 left ╮ ╭ Notes      + ╮
//! │ M  T  W  T  F  S  S                 │ │ ○ Send invoice │ │ Release       │
//! │ 09:00 ┃ Team standup                │ │ ○ Review PR    │ │ – bump        │
//! │ ──●── now ────────────────────────  │ │ ✓ Book train   │ │ Ideas    Thu  │
//! │ 10:30 ┃ Design review        (Join) │ │ + Add a plan   │ │ Lyon     Mon  │
//! ╰─────────────────────────────────────╯ ╰────────────────╯ ╰───────────────╯
//! ```

use super::expanded::glass_tile;
use crate::notes::Notes;
use crate::ui::{
    ActionTarget, FlippedView, Palette, Targets, Weight, attributed_parts, font, label, mono_digits, ns, pill_button,
    rect, remove_all_subviews, rgba, rich_label, set_accessibility_label, shape_view, symbol_button, symbol_view,
};
use notchforge_core::engine::Snapshot;
use notchforge_core::model::{CalendarEvent, CalendarState, ReminderItem};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSColor, NSScrollView, NSTextAlignment, NSTextField, NSTextView};

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

const PAD: f64 = 14.0;
const GAP: f64 = 10.0;
const INSET: f64 = 12.0;
const DAY_MS: i64 = 86_400_000;
/// Plans and earlier notes shown at once.
const PLAN_ROWS: usize = 5;
const NOTE_LINKS: usize = 2;

fn white(a: f64) -> Retained<NSColor> {
    rgba(1.0, 1.0, 1.0, a)
}

/// `"09:30"` in local time.
pub fn clock(ms: i64) -> String {
    let (day, _) = notchforge_providers::calendar::local_day(ms);
    let mins = (ms - day).rem_euclid(DAY_MS) / 60_000;
    format!("{:02}:{:02}", mins / 60, mins % 60)
}

/// When a plan is due, in a few words: "Today, 12:00", "Tomorrow", "Mon 3".
pub fn due_label(due_ms: i64, now_ms: i64) -> String {
    let (today, _) = notchforge_providers::calendar::local_day(now_ms);
    let (due_day, _) = notchforge_providers::calendar::local_day(due_ms);
    let days = (due_day - today).div_euclid(DAY_MS);
    let time = if due_ms != due_day { format!(", {}", clock(due_ms)) } else { String::new() };
    match days {
        d if d < 0 => format!("Overdue{time}"),
        0 => format!("Today{time}"),
        1 => format!("Tomorrow{time}"),
        d => format!("In {d} days"),
    }
}

/// "in 18 min", "in 2h", "now".
fn starts_in(start_ms: i64, now_ms: i64) -> String {
    let mins = (start_ms - now_ms) / 60_000;
    match mins {
        m if m <= 0 => "now".into(),
        1..=59 => format!("in {mins} min"),
        _ => format!("in {}h", (mins + 30) / 60),
    }
}

const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub struct PlannerPage {
    pub root: Retained<FlippedView>,
    mtm: MainThreadMarker,
    p: Palette,
    data_dir: PathBuf,
    day: Retained<FlippedView>,
    plans: Retained<FlippedView>,
    plans_head: Retained<FlippedView>,
    plan_rows: Retained<FlippedView>,
    plan_field: Retained<NSTextField>,
    pomodoro_label: Retained<NSTextField>,
    notes_tile: Retained<FlippedView>,
    note_text: Retained<NSTextView>,
    note_hint: Retained<NSTextField>,
    note_links: Retained<FlippedView>,
    notes: RefCell<Notes>,
    /// The note in the editor, and the text it was given.
    editing: Cell<u64>,
    shown_text: RefCell<String>,
    /// Plans added or ticked here, shown at once until Reminders catches up.
    added: RefCell<Vec<String>>,
    done: RefCell<Vec<String>>,
    last: RefCell<Option<(CalendarState, i64)>>,
    targets: Targets,
    row_targets: Targets,
}

impl PlannerPage {
    pub fn new(mtm: MainThreadMarker, w: f64, h: f64, nh: f64, p: &Palette, data_dir: PathBuf) -> Self {
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        let targets = Targets::default();
        let top = nh + 6.0;
        let body_h = h - top - PAD;
        let module_w = (w - 2.0 * PAD - 3.0 * GAP) / 4.0;

        let day = glass_tile(mtm, rect(PAD, top, 2.0 * module_w + GAP, body_h));
        root.addSubview(&day);

        // ── plans ──
        let px = PAD + 2.0 * (module_w + GAP);
        let plans = glass_tile(mtm, rect(px, top, module_w, body_h));
        let plans_head = FlippedView::new(mtm, rect(0.0, 0.0, module_w, 30.0));
        plans.addSubview(&plans_head);
        let plan_rows = FlippedView::new(mtm, rect(0.0, 30.0, module_w, body_h - 30.0 - 44.0));
        plans.addSubview(&plan_rows);
        let field_box = shape_view(
            mtm,
            rect(INSET - 2.0, body_h - INSET - 30.0, module_w - 2.0 * INSET + 4.0, 30.0),
            &white(0.08),
            10.0,
        );
        if let Some(l) = field_box.layer() {
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&white(0.12).CGColor()));
        }
        field_box.addSubview(&symbol_view(mtm, "plus", 10.0, &white(0.6), rect(8.0, 9.0, 12.0, 12.0), ""));
        let plan_field = NSTextField::textFieldWithString(&ns(""), mtm);
        plan_field.setFrame(rect(24.0, 6.0, module_w - 2.0 * INSET - 150.0, 18.0));
        plan_field.setPlaceholderString(Some(&ns("Add a plan")));
        let pomodoro_label = label(mtm, "", &font(11.0, Weight::Medium), &p.secondary, rect(module_w - 2.0 * INSET - 130.0, 6.0, 110.0, 16.0));
        pomodoro_label.setAlignment(NSTextAlignment::Right);
        field_box.addSubview(&pomodoro_label);
        plan_field.setBordered(false);
        plan_field.setBezeled(false);
        plan_field.setDrawsBackground(false);
        plan_field.setFocusRingType(objc2_app_kit::NSFocusRingType::None);
        plan_field.setFont(Some(&font(12.0, Weight::Regular)));
        plan_field.setTextColor(Some(&p.primary));
        crate::ui::set_accessibility_label(&plan_field, "Add a plan");
        field_box.addSubview(&plan_field);
        plans.addSubview(&field_box);
        root.addSubview(&plans);

        // ── notes ──
        let nx = px + module_w + GAP;
        let notes_tile = glass_tile(mtm, rect(nx, top, module_w, body_h));
        let links_h = NOTE_LINKS as f64 * 30.0;
        let editor_h = body_h - 34.0 - INSET - links_h - 6.0;
        let editor_box =
            shape_view(mtm, rect(INSET, 34.0, module_w - 2.0 * INSET, editor_h), &rgba(1.0, 0.84, 0.4, 0.12), 12.0);
        if let Some(l) = editor_box.layer() {
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&rgba(1.0, 0.84, 0.4, 0.3).CGColor()));
        }
        let scroll: Retained<NSScrollView> = NSTextView::scrollableTextView(mtm);
        scroll.setFrame(rect(4.0, 4.0, module_w - 2.0 * INSET - 8.0, editor_h - 8.0));
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        let note_text: Retained<NSTextView> = scroll
            .documentView()
            .and_then(|v| v.downcast::<NSTextView>().ok())
            .expect("a scrollable text view holds a text view");
        note_text.setDrawsBackground(false);
        note_text.setRichText(false);
        note_text.setFont(Some(&font(12.5, Weight::Regular)));
        note_text.setTextColor(Some(&p.primary));
        note_text.setInsertionPointColor(Some(&p.primary));
        crate::ui::set_accessibility_label(&note_text, "Note");
        let note_hint = label(
            mtm,
            "Type a note…",
            &font(12.5, Weight::Regular),
            &white(0.45),
            rect(10.0, 8.0, module_w - 2.0 * INSET - 20.0, 17.0),
        );
        editor_box.addSubview(&note_hint);
        editor_box.addSubview(&scroll);
        notes_tile.addSubview(&editor_box);
        let note_links = FlippedView::new(mtm, rect(INSET, body_h - INSET - links_h, module_w - 2.0 * INSET, links_h));
        notes_tile.addSubview(&note_links);
        root.addSubview(&notes_tile);

        let notes = Notes::load(&data_dir);
        let this = Self {
            root,
            mtm,
            p: p.clone(),
            data_dir,
            day,
            plans,
            plans_head,
            plan_rows,
            plan_field,
            pomodoro_label,
            notes_tile,
            note_text,
            note_hint,
            note_links,
            notes: RefCell::new(notes),
            editing: Cell::new(0),
            shown_text: RefCell::new(String::new()),
            added: RefCell::default(),
            done: RefCell::default(),
            last: RefCell::new(None),
            targets,
            row_targets: Targets::default(),
        };
        this.wire();
        this.render_notes();
        this
    }

    /// The plan field and the notes' "+" button.
    fn wire(&self) {
        let mtm = self.mtm;
        let t = self.targets.add(mtm, |sender: Option<&AnyObject>| {
            let Some(f) = sender.and_then(|s| s.downcast_ref::<NSTextField>()) else { return };
            let title = f.stringValue().to_string();
            if title.trim().is_empty() {
                return;
            }
            f.setStringValue(&ns(""));
            crate::app::with_controller(|c| c.add_plan(title.trim()));
        });
        // SAFETY: target kept alive by `targets`.
        unsafe {
            self.plan_field.setTarget(Some(t.as_any()));
            self.plan_field.setAction(Some(ActionTarget::action()));
        }
        let t = self.targets.add(mtm, |_| {
            crate::app::with_controller(|c| c.new_note());
        });
        let w = self.notes_tile.frame().size.width;
        let plus =
            symbol_button(mtm, "plus", 10.0, "New note", &self.p.primary, &t, rect(w - INSET - 22.0, 8.0, 22.0, 22.0));
        crate::ui::make_card(&plus, &white(0.14), 11.0);
        self.notes_tile.addSubview(&plus);
        self.notes_tile.addSubview(&label(
            mtm,
            "Notes",
            &font(12.0, Weight::Semibold),
            &self.p.primary,
            rect(INSET, 10.0, w - 2.0 * INSET - 30.0, 15.0),
        ));
    }

    pub fn update(&self, snap: &Snapshot) {
        self.save_note();
        let cal = &snap.calendar;
        // Redraw the calendar and plans when they change, or every minute for the now-line.
        let minute = snap.now_ms / 60_000;
        let same = self.last.borrow().as_ref().is_some_and(|(c, m)| c == cal && *m == minute);
        if same {
            return;
        }
        *self.last.borrow_mut() = Some((cal.clone(), minute));
        // What Reminders now has supersedes what was added or ticked here.
        self.added.borrow_mut().retain(|t| !cal.reminders.iter().any(|r| &r.title == t));
        self.done.borrow_mut().retain(|id| cal.reminders.iter().any(|r| &r.id == id));
        self.render_day(cal, snap.now_ms);
        self.render_plans(cal, snap.now_ms);
        if snap.pomodoro_rounds > 0 {
            crate::ui::set_text(
                &self.pomodoro_label,
                &format!("{} pomodoro{} today", snap.pomodoro_rounds, if snap.pomodoro_rounds == 1 { "" } else { "s" }),
            );
        }
    }

    /// A plan was added here: show it until Reminders lists it.
    pub fn plan_added(&self, title: &str) {
        self.added.borrow_mut().push(title.to_owned());
        self.rerender_plans();
    }

    /// A plan was ticked here: hide it until Reminders drops it.
    pub fn plan_done(&self, id: &str) {
        self.done.borrow_mut().push(id.to_owned());
        self.rerender_plans();
    }

    fn rerender_plans(&self) {
        let last = self.last.borrow().clone();
        if let Some((cal, minute)) = last {
            self.render_plans(&cal, minute * 60_000);
        }
    }

    fn render_day(&self, cal: &CalendarState, now: i64) {
        let mtm = self.mtm;
        let p = &self.p;
        let tile = &self.day;
        remove_all_subviews(tile);
        let size = tile.frame().size;
        let (w, h) = (size.width, size.height);
        let inner = w - 2.0 * INSET;

        // Saturday 7 November ··· 4 events
        let (_, weekday) = notchforge_providers::calendar::local_day(now);
        let date = cal.week_dates.get(weekday as usize).copied().unwrap_or(0);
        let month = month_of(now);
        let head = attributed_parts(&[
            (WEEKDAYS[weekday as usize % 7], &font(20.0, Weight::Bold), &p.primary),
            (&format!("  {date} {}", MONTHS[month]), &font(13.0, Weight::Regular), &white(0.72)),
        ]);
        tile.addSubview(&rich_label(mtm, &head, rect(INSET + 2.0, 10.0, inner - 80.0, 26.0)));
        let count = match cal.today.len() {
            0 => "Free today".to_owned(),
            1 => "1 event".to_owned(),
            n => format!("{n} events"),
        };
        let c =
            label(mtm, &count, &font(11.5, Weight::Regular), &white(0.72), rect(w - INSET - 90.0, 17.0, 90.0, 15.0));
        c.setAlignment(NSTextAlignment::Right);
        tile.addSubview(&c);

        if !cal.granted {
            self.quiet(tile, "calendar", "Allow Calendar access in Settings › Permissions to see your day.");
            return;
        }

        // The week, with today in red and a dot under busy days.
        let cell = inner / 7.0;
        for (i, letter) in ["M", "T", "W", "T", "F", "S", "S"].iter().enumerate() {
            let x = INSET + i as f64 * cell;
            let l = label(mtm, letter, &font(10.5, Weight::Regular), &white(0.72), rect(x, 42.0, cell, 13.0));
            l.setAlignment(NSTextAlignment::Center);
            tile.addSubview(&l);
            let today = i == weekday as usize;
            if today {
                tile.addSubview(&shape_view(
                    mtm,
                    rect(x + (cell - 24.0) / 2.0, 56.0, 24.0, 24.0),
                    &rgba(1.0, 0.27, 0.23, 1.0),
                    12.0,
                ));
            }
            let n = cal.week_dates.get(i).map_or(String::new(), |d| d.to_string());
            let d = label(mtm, &n, &mono_digits(12.0, Weight::Semibold), &p.primary, rect(x, 60.0, cell, 16.0));
            d.setAlignment(NSTextAlignment::Center);
            tile.addSubview(&d);
            if cal.busy_days.get(i).copied().unwrap_or(false) && !today {
                tile.addSubview(&shape_view(mtm, rect(x + cell / 2.0 - 2.0, 82.0, 4.0, 4.0), &white(0.7), 2.0));
            }
        }
        tile.addSubview(&shape_view(mtm, rect(INSET, 92.0, inner, 0.5), &white(0.12), 0.0));

        // Today's events, with a line where now falls among them.
        let top = 98.0;
        let room = h - top - INSET;
        if cal.today.is_empty() {
            let l = label(
                mtm,
                "Nothing on the calendar today",
                &font(12.5, Weight::Regular),
                &p.secondary,
                rect(INSET, top + room / 2.0 - 9.0, inner, 18.0),
            );
            l.setAlignment(NSTextAlignment::Center);
            tile.addSubview(&l);
            return;
        }
        let events: Vec<&CalendarEvent> = cal.today.iter().take(5).collect();
        let now_at = events.iter().position(|e| e.starts_at_ms > now).unwrap_or(events.len());
        let line_h = if now_at > 0 && now_at < events.len() { 14.0 } else { 0.0 };
        let row_h = ((room - line_h) / events.len() as f64).min(48.0);
        let mut y = top;
        for (i, e) in events.iter().enumerate() {
            if i == now_at && line_h > 0.0 {
                self.now_line(tile, y + 3.0, inner, now);
                y += line_h;
            }
            self.event_row(tile, e, y, row_h, inner, now);
            y += row_h;
        }
    }

    fn now_line(&self, tile: &FlippedView, y: f64, inner: f64, now: i64) {
        let mtm = self.mtm;
        let red = rgba(1.0, 0.27, 0.23, 1.0);
        let t = label(mtm, &clock(now), &mono_digits(10.0, Weight::Bold), &red, rect(INSET, y - 3.0, 40.0, 13.0));
        t.setAlignment(NSTextAlignment::Right);
        tile.addSubview(&t);
        tile.addSubview(&shape_view(mtm, rect(INSET + 46.0, y + 0.5, 7.0, 7.0), &red, 3.5));
        tile.addSubview(&shape_view(mtm, rect(INSET + 52.0, y + 3.25, inner - 52.0, 1.5), &red, 0.75));
    }

    fn event_row(&self, tile: &FlippedView, e: &CalendarEvent, y: f64, h: f64, inner: f64, now: i64) {
        let mtm = self.mtm;
        let p = &self.p;
        let past = e.ends_at_ms < now;
        let fade = |c: Retained<NSColor>| if past { c.colorWithAlphaComponent(0.5) } else { c };
        let cy = y + (h - 32.0) / 2.0;
        let (start, end) = if e.all_day {
            ("All day".to_owned(), String::new())
        } else {
            (clock(e.starts_at_ms), clock(e.ends_at_ms))
        };
        let s = label(
            mtm,
            &start,
            &mono_digits(12.0, Weight::Semibold),
            &fade(p.primary.clone()),
            rect(INSET, cy, 40.0, 16.0),
        );
        s.setAlignment(NSTextAlignment::Right);
        tile.addSubview(&s);
        let en = label(
            mtm,
            &end,
            &mono_digits(10.5, Weight::Regular),
            &fade(white(0.56)),
            rect(INSET, cy + 16.0, 40.0, 14.0),
        );
        en.setAlignment(NSTextAlignment::Right);
        tile.addSubview(&en);
        let (r, g, b) = e.color.unwrap_or((10, 132, 255));
        tile.addSubview(&shape_view(
            mtm,
            rect(INSET + 50.0, cy + 1.0, 3.0, 30.0),
            &fade(rgba(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, 1.0)),
            1.5,
        ));
        // A meeting starting within 15 minutes, or under way, can be joined.
        let joinable = e.meeting_url.is_some() && now + 15 * 60_000 >= e.starts_at_ms && now <= e.ends_at_ms;
        let text_x = INSET + 62.0;
        let text_w = inner - 62.0 - if joinable { 56.0 } else { 0.0 };
        tile.addSubview(&label(
            mtm,
            &e.title,
            &font(13.0, Weight::Semibold),
            &fade(p.primary.clone()),
            rect(text_x, cy, text_w, 17.0),
        ));
        let mut meta = e.location.clone().filter(|l| !l.starts_with("http")).unwrap_or_else(|| e.calendar.clone());
        if e.starts_at_ms > now && !e.all_day {
            meta = format!("{meta} · {}", starts_in(e.starts_at_ms, now));
        }
        tile.addSubview(&label(
            mtm,
            &meta,
            &font(11.5, Weight::Regular),
            &fade(white(0.72)),
            rect(text_x, cy + 17.0, text_w, 15.0),
        ));
        if let Some(url) = e.meeting_url.clone().filter(|_| joinable) {
            let t = self.row_targets.add(mtm, move |_| {
                crate::sharing::open_url(&url);
            });
            tile.addSubview(&pill_button(
                mtm,
                "Join",
                None,
                &font(12.0, Weight::Semibold),
                &NSColor::whiteColor(),
                &rgba(0.039, 0.518, 1.0, 1.0),
                12.0,
                &t,
                rect(INSET + inner - 50.0, cy + 4.0, 50.0, 24.0),
            ));
        }
        set_accessibility_label(&s, &format!("{}, {start} to {end}", e.title));
    }

    fn quiet(&self, tile: &FlippedView, symbol: &str, text: &str) {
        let size = tile.frame().size;
        tile.addSubview(&symbol_view(
            self.mtm,
            symbol,
            22.0,
            &white(0.5),
            rect((size.width - 28.0) / 2.0, size.height / 2.0 - 30.0, 28.0, 28.0),
            "",
        ));
        let l = crate::ui::wrapping_label(
            self.mtm,
            text,
            &font(12.0, Weight::Regular),
            &self.p.secondary,
            rect(INSET, size.height / 2.0 + 4.0, size.width - 2.0 * INSET, 34.0),
            2,
        );
        l.setAlignment(NSTextAlignment::Center);
        tile.addSubview(&l);
    }

    fn render_plans(&self, cal: &CalendarState, now: i64) {
        let mtm = self.mtm;
        let p = &self.p;
        self.row_targets.clear();
        remove_all_subviews(&self.plan_rows);
        remove_all_subviews(&self.plans_head);
        let done = self.done.borrow();
        let mut items: Vec<ReminderItem> = cal.reminders.iter().filter(|r| !done.contains(&r.id)).cloned().collect();
        for t in self.added.borrow().iter() {
            items.insert(0, ReminderItem { id: String::new(), title: t.clone(), due_at_ms: None });
        }
        let w = self.plans.frame().size.width;
        self.plans_head.addSubview(&label(
            mtm,
            "Plans",
            &font(12.0, Weight::Semibold),
            &p.primary,
            rect(INSET, 10.0, w / 2.0, 15.0),
        ));
        let left = match items.len() {
            0 => String::new(),
            n => format!("{n} left"),
        };
        let c =
            label(mtm, &left, &font(11.0, Weight::Regular), &white(0.72), rect(w / 2.0, 11.0, w / 2.0 - INSET, 14.0));
        c.setAlignment(NSTextAlignment::Right);
        self.plans_head.addSubview(&c);
        if !cal.granted {
            self.quiet(&self.plan_rows, "checklist", "Allow Reminders access to see your plans.");
            return;
        }
        if items.is_empty() {
            self.quiet(&self.plan_rows, "checkmark.circle", "Nothing planned. Add one below.");
            return;
        }
        let rows_h = self.plan_rows.frame().size.height;
        let step = (rows_h / PLAN_ROWS as f64).min(46.0);
        for (i, r) in items.iter().take(PLAN_ROWS).enumerate() {
            let y = i as f64 * step;
            if i > 0 {
                self.plan_rows.addSubview(&shape_view(mtm, rect(INSET, y, w - 2.0 * INSET, 0.5), &white(0.1), 0.0));
            }
            let cy = y + (step - 30.0) / 2.0;
            let id = r.id.clone();
            let t = self.row_targets.add(mtm, move |_| {
                if !id.is_empty() {
                    crate::app::with_controller(|c| c.complete_plan(&id));
                }
            });
            let check = symbol_button(
                mtm,
                "circle",
                15.0,
                &format!("Done: {}", r.title),
                &white(0.6),
                &t,
                rect(INSET - 4.0, cy + 2.0, 26.0, 26.0),
            );
            self.plan_rows.addSubview(&check);
            let tx = INSET + 26.0;
            let due = r.due_at_ms.map(|d| due_label(d, now));
            let overdue_or_today = r.due_at_ms.is_some_and(|d| {
                d < now + DAY_MS && due.as_deref().is_some_and(|l| l.starts_with("Today") || l.starts_with("Overdue"))
            });
            let title_y = if due.is_some() { cy } else { cy + 7.0 };
            self.plan_rows.addSubview(&label(
                mtm,
                &r.title,
                &font(12.5, Weight::Regular),
                &p.primary,
                rect(tx, title_y, w - tx - INSET, 16.0),
            ));
            if let Some(d) = due {
                let color = if overdue_or_today { rgba(1.0, 0.624, 0.039, 1.0) } else { white(0.56) };
                self.plan_rows.addSubview(&label(
                    mtm,
                    &d,
                    &font(11.0, Weight::Regular),
                    &color,
                    rect(tx, cy + 16.0, w - tx - INSET, 14.0),
                ));
            }
        }
    }

    /// The note being edited fills the editor; the next ones are links below it.
    fn render_notes(&self) {
        let mtm = self.mtm;
        let notes = self.notes.borrow();
        let first = notes.notes.first();
        self.editing.set(first.map_or(0, |n| n.id));
        let text = first.map_or(String::new(), |n| n.text.clone());
        self.note_text.setString(&ns(&text));
        self.note_hint.setHidden(!text.is_empty());
        *self.shown_text.borrow_mut() = text;
        remove_all_subviews(&self.note_links);
        let w = self.note_links.frame().size.width;
        for (i, n) in notes.notes.iter().skip(1).take(NOTE_LINKS).enumerate() {
            let id = n.id;
            let t = self.row_targets.add(mtm, move |_| {
                crate::app::with_controller(|c| c.open_note(id));
            });
            let row = shape_view(mtm, rect(0.0, i as f64 * 30.0, w, 26.0), &white(0.06), 8.0);
            row.addSubview(&label(
                mtm,
                n.title(),
                &font(11.5, Weight::Regular),
                &self.p.primary,
                rect(8.0, 5.0, w - 60.0, 15.0),
            ));
            let when = label(
                mtm,
                &note_age(n.updated_ms),
                &font(11.0, Weight::Regular),
                &white(0.56),
                rect(w - 50.0, 5.0, 42.0, 15.0),
            );
            when.setAlignment(NSTextAlignment::Right);
            row.addSubview(&when);
            row.addSubview(&crate::ui::click_overlay(
                mtm,
                &t,
                rect(0.0, 0.0, w, 26.0),
                &format!("Open note {}", n.title()),
            ));
            self.note_links.addSubview(&row);
        }
    }

    /// Keep what was typed: saved when it changes.
    pub fn save_note(&self) {
        let text = self.note_text.string().to_string();
        self.note_hint.setHidden(!text.is_empty());
        if *self.shown_text.borrow() == text {
            return;
        }
        let now = notchforge_core::time::now_ms();
        let mut notes = self.notes.borrow_mut();
        if notes.notes.is_empty() {
            let id = notes.create(now);
            self.editing.set(id);
        }
        notes.set_text(self.editing.get(), &text, now);
        notes.save(&self.data_dir);
        *self.shown_text.borrow_mut() = text;
    }

    pub fn new_note(&self) {
        self.save_note();
        {
            let mut notes = self.notes.borrow_mut();
            // An empty note on top is reused rather than stacked.
            if notes.notes.first().is_none_or(|n| !n.text.trim().is_empty()) {
                notes.create(notchforge_core::time::now_ms());
            }
        }
        self.render_notes();
        if let Some(w) = self.note_text.window() {
            w.makeFirstResponder(Some(&self.note_text));
        }
    }

    pub fn open_note(&self, id: u64) {
        self.save_note();
        self.notes.borrow_mut().open(id);
        self.notes.borrow().save(&self.data_dir);
        self.render_notes();
    }
}

/// "Thu", "Mon", "12 Oct": a note's last change.
fn note_age(ms: i64) -> String {
    let now = notchforge_core::time::now_ms();
    let (today, _) = notchforge_providers::calendar::local_day(now);
    let (day, weekday) = notchforge_providers::calendar::local_day(ms);
    match (today - day) / DAY_MS {
        0 => clock(ms),
        1..=6 => WEEKDAYS[weekday as usize % 7][..3].to_owned(),
        _ => format!("{} {}", day_of_month(ms), &MONTHS[month_of(ms)][..3]),
    }
}

fn day_of_month(ms: i64) -> u32 {
    tm_of(ms).tm_mday as u32
}

fn month_of(ms: i64) -> usize {
    tm_of(ms).tm_mon.clamp(0, 11) as usize
}

fn tm_of(ms: i64) -> libc::tm {
    // SAFETY: localtime_r fills a caller-owned struct.
    unsafe {
        let t = (ms / 1000) as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_dates_read_plainly() {
        let now = notchforge_core::time::now_ms();
        let (today, _) = notchforge_providers::calendar::local_day(now);
        assert_eq!(due_label(today, now), "Today");
        assert_eq!(due_label(today + 12 * 3_600_000, now), "Today, 12:00");
        assert_eq!(due_label(today + DAY_MS, now), "Tomorrow");
        assert_eq!(due_label(today - DAY_MS, now), "Overdue");
        assert_eq!(due_label(today + 4 * DAY_MS, now), "In 4 days");
    }

    #[test]
    fn meetings_say_when_they_start() {
        assert_eq!(starts_in(18 * 60_000, 0), "in 18 min");
        assert_eq!(starts_in(130 * 60_000, 0), "in 2h");
        assert_eq!(starts_in(0, 5), "now");
    }
}
