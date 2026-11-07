//! First-run setup: every permission the app can use, why, and whether it is
//! granted — ticking itself off live while System Settings is open — plus
//! one-click connection of the coding agents found on this Mac.
//!
//! Shown once on first launch (until `general.onboarded`), and any time from
//! the menu bar's "Set Up Notchisland…".

use crate::app;
use crate::ui::{
    FlippedView, Targets, Weight, font, label, ns, pill_button, rect, rgba, shape_view, symbol_view, wrapping_label,
};
use notchforge_core::model::{Permission, PermissionState};
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSBackingStoreType, NSColor, NSTextAlignment, NSWindow, NSWindowStyleMask,
    NSWindowTitleVisibility,
};
use std::cell::RefCell;

const W: f64 = 600.0;
const H: f64 = 690.0;
const PAD: f64 = 28.0;
const ROW_H: f64 = 64.0;

/// A permission's place in the setup: what it unlocks, in plain words.
pub struct Step {
    pub permission: Permission,
    pub title: &'static str,
    pub why: &'static str,
    pub symbol: &'static str,
}

pub const STEPS: [Step; 6] = [
    Step {
        permission: Permission::Accessibility,
        title: "Accessibility",
        why: "Answer and end calls, reply to message banners, and read video timelines in your browser.",
        symbol: "accessibility",
    },
    Step {
        permission: Permission::FullDiskAccess,
        title: "Full Disk Access",
        why: "Read notifications from Notification Center so they appear in the notch.",
        symbol: "internaldrive",
    },
    Step {
        permission: Permission::Automation,
        title: "Automation",
        why: "Show and control Spotify, Music and the video playing in your browser.",
        symbol: "gearshape.2",
    },
    Step {
        permission: Permission::Calendar,
        title: "Calendars & Reminders",
        why: "Your day, your next meeting with a Join button, and reminders you can tick off.",
        symbol: "calendar",
    },
    Step {
        permission: Permission::Location,
        title: "Location",
        why: "Local weather and rain alerts. A city in Settings works too.",
        symbol: "location",
    },
    Step {
        permission: Permission::Notifications,
        title: "Notifications",
        why: "Alerts from the app itself, such as timers, while the notch is hidden.",
        symbol: "bell.badge",
    },
];

thread_local! {
    static WINDOW: RefCell<Option<Onboarding>> = const { RefCell::new(None) };
}

struct Onboarding {
    window: Retained<NSWindow>,
    targets: Targets,
    timer: Option<Retained<objc2_foundation::NSTimer>>,
    /// What was last drawn, so the live refresh only redraws on a change.
    drawn: Vec<PermissionState>,
}

/// Open the setup window (or bring it forward).
pub fn show() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    WINDOW.with(|w| {
        let mut w = w.borrow_mut();
        let ob = w.get_or_insert_with(|| Onboarding::new(mtm));
        ob.render(mtm);
        ob.window.center();
        ob.window.makeKeyAndOrderFront(None);
        ob.start_refresh();
    });
    // SAFETY: activating our own app so the window comes forward.
    #[allow(deprecated)]
    objc2_app_kit::NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
}

/// First launch: show it once, unless setup already happened.
pub fn show_if_first_run() {
    if !app::with_config(|c| c.general.onboarded).unwrap_or(true) {
        show();
    }
}

fn finish() {
    WINDOW.with(|w| {
        if let Some(ob) = w.borrow_mut().as_mut() {
            if let Some(t) = ob.timer.take() {
                t.invalidate();
            }
            ob.window.orderOut(None);
        }
    });
    if let Some(mut cfg) = app::with_config(|c| c.clone()) {
        if !cfg.general.onboarded {
            cfg.general.onboarded = true;
            app::send(notchforge_core::engine::Command::UpdateConfig(Box::new(cfg)));
        }
    }
}

fn states() -> Vec<PermissionState> {
    STEPS.iter().map(|s| crate::permissions::state(s.permission)).collect()
}

/// How far along setup is: granted (or not needed) out of all.
pub fn progress(states: &[PermissionState]) -> (usize, usize) {
    let done = states.iter().filter(|s| matches!(s, PermissionState::Granted | PermissionState::NotRequired)).count();
    (done, states.len())
}

impl Onboarding {
    fn new(mtm: MainThreadMarker) -> Self {
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::FullSizeContentView;
        // SAFETY: NSWindow designated initializer.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, W, H),
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&ns("Set Up Notchisland"));
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setBackgroundColor(Some(&rgba(0.07, 0.07, 0.08, 1.0)));
        // SAFETY: AppKit's appearance-name constant.
        if let Some(dark) = NSAppearance::appearanceNamed(unsafe { objc2_app_kit::NSAppearanceNameDarkAqua }) {
            window.setAppearance(Some(&dark));
        }
        // SAFETY: we keep the window alive ourselves.
        unsafe { window.setReleasedWhenClosed(false) };
        Self { window, targets: Targets::default(), timer: None, drawn: Vec::new() }
    }

    /// Re-check every second while open: granting in System Settings ticks
    /// the row off without touching this window.
    fn start_refresh(&mut self) {
        if self.timer.is_some() {
            return;
        }
        let block = block2::RcBlock::new(|_t: std::ptr::NonNull<objc2_foundation::NSTimer>| {
            let Some(mtm) = MainThreadMarker::new() else { return };
            WINDOW.with(|w| {
                if let Some(ob) = w.borrow_mut().as_mut() {
                    if !ob.window.isVisible() {
                        if let Some(t) = ob.timer.take() {
                            t.invalidate();
                        }
                    } else if states() != ob.drawn {
                        ob.render(mtm);
                    }
                }
            });
        });
        // SAFETY: documented NSTimer API.
        let t = unsafe { objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_repeats_block(1.0, true, &block) };
        self.timer = Some(t);
    }

    fn render(&mut self, mtm: MainThreadMarker) {
        self.targets.clear();
        let current = states();
        let root = build(mtm, &self.targets, &current);
        self.window.setContentView(Some(&root));
        self.drawn = current;
    }
}

/// The window's content for these states (also rendered by the self-test).
pub fn build(mtm: MainThreadMarker, targets: &Targets, states: &[PermissionState]) -> Retained<FlippedView> {
    let white = |a: f64| rgba(1.0, 1.0, 1.0, a);
    let root = FlippedView::new(mtm, rect(0.0, 0.0, W, H));
    root.setWantsLayer(true);
    if let Some(l) = root.layer() {
        let g = crate::ui::gradient_layer(
            &[(0.10, 0.07, 0.12, 1.0), (0.04, 0.04, 0.05, 1.0)],
            objc2_foundation::NSPoint::new(0.5, 0.0),
            objc2_foundation::NSPoint::new(0.5, 1.0),
        );
        g.setFrame(rect(0.0, 0.0, W, H));
        l.addSublayer(&g);
    }
    let (done, total) = progress(states);
    let mut y = 44.0;
    if let Some(logo) = crate::icons::app_logo() {
        if let Some(v) = crate::ui::image_box(mtm, rect(PAD, y, 44.0, 44.0), &logo) {
            root.addSubview(&v);
        }
    }
    root.addSubview(&label(
        mtm,
        "Welcome to Notchisland",
        &font(24.0, Weight::Bold),
        &NSColor::whiteColor(),
        rect(PAD + 56.0, y - 2.0, W - 2.0 * PAD - 56.0, 30.0),
    ));
    root.addSubview(&label(
        mtm,
        &format!("Each permission is optional and unlocks one part of the notch · {done} of {total} ready"),
        &font(13.0, Weight::Regular),
        &white(0.62),
        rect(PAD + 56.0, y + 28.0, W - 2.0 * PAD - 56.0, 18.0),
    ));
    y += 70.0;
    // Progress bar.
    let bar = shape_view(mtm, rect(PAD, y, W - 2.0 * PAD, 4.0), &white(0.12), 2.0);
    let fill_w = (W - 2.0 * PAD) * done as f64 / total.max(1) as f64;
    bar.addSubview(&shape_view(mtm, rect(0.0, 0.0, fill_w, 4.0), &rgba(0.2, 0.84, 0.39, 1.0), 2.0));
    root.addSubview(&bar);
    y += 18.0;

    for (step, state) in STEPS.iter().zip(states) {
        let row = shape_view(mtm, rect(PAD, y, W - 2.0 * PAD, ROW_H), &white(0.06), 14.0);
        let granted = matches!(state, PermissionState::Granted | PermissionState::NotRequired);
        let icon_bg = if granted { rgba(0.2, 0.84, 0.39, 0.22) } else { white(0.1) };
        let icon_tile = shape_view(mtm, rect(12.0, 12.0, 40.0, 40.0), &icon_bg, 10.0);
        icon_tile.addSubview(&symbol_view(
            mtm,
            if granted { "checkmark" } else { step.symbol },
            17.0,
            &*if granted { rgba(0.3, 0.9, 0.48, 1.0) } else { white(0.9) },
            rect(10.0, 10.0, 20.0, 20.0),
            step.title,
        ));
        row.addSubview(&icon_tile);
        let tw = W - 2.0 * PAD - 64.0 - 130.0;
        row.addSubview(&label(
            mtm,
            step.title,
            &font(14.0, Weight::Semibold),
            &NSColor::whiteColor(),
            rect(64.0, 11.0, tw, 18.0),
        ));
        row.addSubview(&wrapping_label(
            mtm,
            step.why,
            &font(11.5, Weight::Regular),
            &white(0.6),
            rect(64.0, 29.0, tw, 30.0),
            2,
        ));
        let bx = W - 2.0 * PAD - 12.0 - 110.0;
        if granted {
            let l = label(
                mtm,
                crate::permissions::state_label(*state),
                &font(12.5, Weight::Medium),
                &rgba(0.3, 0.9, 0.48, 1.0),
                rect(bx, 23.0, 110.0, 18.0),
            );
            l.setAlignment(NSTextAlignment::Right);
            row.addSubview(&l);
        } else {
            let p = step.permission;
            let t = targets.add(mtm, move |_| crate::permissions::request(p));
            let title = if *state == PermissionState::NotDetermined { "Allow" } else { "Open Settings" };
            row.addSubview(&pill_button(
                mtm,
                title,
                None,
                &font(12.5, Weight::Semibold),
                &NSColor::whiteColor(),
                &rgba(0.039, 0.518, 1.0, 1.0),
                14.0,
                &t,
                rect(bx, 18.0, 110.0, 28.0),
            ));
        }
        root.addSubview(&row);
        y += ROW_H + 8.0;
    }

    // Coding agents: one click each, in Settings › Agents.
    y += 6.0;
    root.addSubview(&label(
        mtm,
        "Coding agents",
        &font(14.0, Weight::Semibold),
        &NSColor::whiteColor(),
        rect(PAD, y, 300.0, 18.0),
    ));
    root.addSubview(&wrapping_label(
        mtm,
        "See live sessions and their time left, and allow or deny tool requests from the notch. Works with Claude Code, Codex, Gemini CLI, Cursor, OpenCode and Aider.",
        &font(11.5, Weight::Regular),
        &white(0.6),
        rect(PAD, y + 20.0, W - 2.0 * PAD - 150.0, 46.0),
        3,
    ));
    let t = targets.add(mtm, |_| app::show_settings_pane(crate::settings::Pane::Agents));
    root.addSubview(&pill_button(
        mtm,
        "Connect agents",
        None,
        &font(12.5, Weight::Semibold),
        &NSColor::whiteColor(),
        &white(0.14),
        14.0,
        &t,
        rect(W - PAD - 130.0, y + 8.0, 130.0, 28.0),
    ));

    let t = targets.add(mtm, |_| finish());
    root.addSubview(&pill_button(
        mtm,
        if done == total { "Done" } else { "Continue" },
        None,
        &font(13.0, Weight::Semibold),
        &rgba(0.06, 0.06, 0.07, 1.0),
        &NSColor::whiteColor(),
        16.0,
        &t,
        rect(W - PAD - 120.0, H - PAD - 32.0, 120.0, 32.0),
    ));
    root.addSubview(&label(
        mtm,
        "You can change any of this later in Settings › Permissions.",
        &font(11.5, Weight::Regular),
        &white(0.45),
        rect(PAD, H - PAD - 24.0, W - 2.0 * PAD - 140.0, 16.0),
    ));
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_counts_granted_and_unneeded() {
        let s = [PermissionState::Granted, PermissionState::Denied, PermissionState::NotRequired];
        assert_eq!(progress(&s), (2, 3));
        assert_eq!(STEPS.len(), 6);
    }
}
