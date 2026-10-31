//! End-to-end UI self-test: `notchforge --self-test <dir>`.
//!
//! Drives the *real* running app through its interactive paths and writes
//! `<dir>/selftest.json` plus a screenshot per stage, then exits with status 0
//! (all checks passed) or 1. Used by CI on a macOS runner.
//!
//! Input is first attempted with synthetic CoreGraphics events (pointer move,
//! click, Escape), which exercise the global monitors, tracking areas and
//! `RootView` exactly like a user would. Posting synthetic events needs the
//! Accessibility permission; when the host does not grant it, the test falls
//! back to the controller's own input entry points (`pointer_moved`,
//! `root_mouse_down`, `key_down`), which are what those AppKit callbacks call,
//! and records which path was used for each step.

use crate::app::{self, with_controller};
use crate::controller::AnimStats;
use crate::ffi::cf::Owned;
use crate::ffi::cg;
use block2::RcBlock;
use notchforge_core::apps::{AppRef, KnownApp};
use notchforge_core::engine::Command;
use notchforge_core::geometry::{LayoutKind, Rect};
use notchforge_core::machine::State;
use notchforge_core::model::{NewNotification, NotificationKind, Origin, Permission, Priority};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEventModifierFlags, NSScreen};
use objc2_foundation::{NSPoint, NSTimer};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;

const KEY_ESCAPE: u16 = 53;

#[derive(Default)]
struct Report {
    checks: Vec<Value>,
    input: Vec<Value>,
    metrics: serde_json::Map<String, Value>,
    screenshots: Vec<String>,
    failed: bool,
}

impl Report {
    fn check(&mut self, name: &str, passed: bool, detail: impl Into<String>) {
        let detail = detail.into();
        tracing::info!(check = name, passed, %detail, "self-test");
        self.failed |= !passed;
        self.checks.push(json!({ "name": name, "passed": passed, "detail": detail }));
    }
    fn input(&mut self, step: &str, synthetic_worked: bool) {
        self.input.push(
            json!({ "step": step, "path": if synthetic_worked { "synthetic CGEvent" } else { "controller fallback" } }),
        );
    }
}

struct Ctx {
    dir: PathBuf,
    report: RefCell<Report>,
}

fn rect_str(r: &Rect) -> String {
    format!("{:.1},{:.1} {:.1}×{:.1}", r.x, r.y, r.width, r.height)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1.0
}

/// Height of the primary display, for AppKit (bottom-left) ↔ CG (top-left) conversion.
fn primary_height(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm).iter().next().map_or(0.0, |s| s.frame().size.height)
}

fn post_mouse(mtm: MainThreadMarker, kind: u32, p: NSPoint) {
    let pt = cg::CGPoint { x: p.x, y: primary_height(mtm) - p.y };
    // SAFETY: CGEvent create/post/release with valid arguments.
    unsafe {
        if let Some(ev) =
            Owned::from_create(cg::CGEventCreateMouseEvent(std::ptr::null(), kind, pt, cg::kCGMouseButtonLeft))
        {
            cg::CGEventPost(cg::kCGHIDEventTap, ev.as_ptr());
        }
    }
}

fn post_key(code: u16) {
    // SAFETY: as above.
    unsafe {
        for down in [true, false] {
            if let Some(ev) = Owned::from_create(cg::CGEventCreateKeyboardEvent(std::ptr::null(), code, down)) {
                cg::CGEventPost(cg::kCGHIDEventTap, ev.as_ptr());
            }
        }
    }
}

/// Region screenshot (`screencapture` uses top-left coordinates in points).
fn screenshot(ctx: &Ctx, mtm: MainThreadMarker, name: &str, region: Option<Rect>) {
    let path = ctx.dir.join(format!("{name}.png"));
    let mut cmd = std::process::Command::new("/usr/sbin/screencapture");
    cmd.arg("-x");
    if let Some(r) = region {
        let top = primary_height(mtm) - r.max_y();
        cmd.arg("-R").arg(format!("{:.0},{:.0},{:.0},{:.0}", r.x, top.max(0.0), r.width, r.height));
    }
    let ok = cmd.arg(&path).status().is_ok_and(|s| s.success()) && path.exists();
    if ok {
        ctx.report.borrow_mut().screenshots.push(path.display().to_string());
    } else {
        tracing::warn!(name, "screenshot unavailable (Screen Recording permission?)");
    }
}

fn anim_json(a: AnimStats) -> Value {
    json!({
        "frames": a.frames,
        "duration_ms": (a.duration_secs * 1000.0).round(),
        "fps": (a.fps() * 10.0).round() / 10.0,
        "max_frame_ms": (a.max_frame_secs * 1000.0 * 10.0).round() / 10.0,
    })
}

/// Settled-geometry checks shared by several stages.
fn check_settled(ctx: &Ctx, stage: &str, expect: &[LayoutKind]) {
    let Some((kind, island, panel, layout, animating)) =
        with_controller(|c| (c.layout_kind(), c.island_global(), c.panel_global(), c.layout(), c.is_animating()))
    else {
        ctx.report.borrow_mut().check(&format!("{stage}: controller alive"), false, "no controller");
        return;
    };
    let mut r = ctx.report.borrow_mut();
    r.check(&format!("{stage}: layout"), expect.contains(&kind), format!("{kind:?}"));
    r.check(&format!("{stage}: animation settled"), !animating, "");
    let shape = layout.shape(kind);
    r.check(
        &format!("{stage}: island size matches layout"),
        close(island.width, shape.width) && close(island.height, shape.height),
        format!("island {} vs shape {:.1}×{:.1}", rect_str(&island), shape.width, shape.height),
    );
    r.check(
        &format!("{stage}: island pinned to top edge, centred on notch"),
        close(island.max_y(), layout.screen.frame.max_y()) && close(island.mid_x(), layout.notch.rect.mid_x()),
        rect_str(&island),
    );
    r.check(
        &format!("{stage}: window shrunk to island"),
        close(panel.width, island.width) && close(panel.height, island.height),
        format!("panel {}", rect_str(&panel)),
    );
}

fn run_step(ctx: &Rc<Ctx>, mtm: MainThreadMarker, step: usize) {
    let notch = with_controller(|c| c.layout().notch.rect).unwrap_or_default();
    let center = NSPoint::new(notch.mid_x(), notch.y + notch.height / 2.0);
    let state = || with_controller(|c| c.machine_state());
    match step {
        // ── collapsed at rest ──
        0 => {
            let Some((layout, ignores)) = with_controller(|c| (c.layout(), c.ignores_mouse())) else { return };
            {
                let mut r = ctx.report.borrow_mut();
                r.metrics.insert(
                    "screen".into(),
                    json!({
                        "frame": rect_str(&layout.screen.frame),
                        "physical_notch": layout.notch.physical,
                        "notch": rect_str(&layout.notch.rect),
                    }),
                );
                r.check("collapsed: click-through", ignores, "");
                r.metrics.insert("system_reduce_motion".into(), json!(crate::views::system_reduce_motion()));
            }
            check_settled(ctx, "collapsed", &[LayoutKind::Collapsed, LayoutKind::CollapsedWithWings]);
            screenshot(ctx, mtm, "01-collapsed", Some(notch.outset(300.0, 300.0, 120.0, 0.0)));
        }
        // ── hover: move the pointer onto the notch ──
        1 => {
            for dx in [-40.0, -10.0, 0.0] {
                post_mouse(mtm, cg::kCGEventMouseMoved, NSPoint::new(center.x + dx, center.y));
            }
        }
        2 => {
            let real = matches!(state(), Some(State::Hover));
            ctx.report.borrow_mut().input("hover", real);
            if !real {
                with_controller(|c| c.pointer_moved(center));
            }
        }
        3 => {
            let (st, ignores, anim) = with_controller(|c| (c.machine_state(), c.ignores_mouse(), c.last_animation()))
                .unwrap_or((State::Collapsed, true, AnimStats::default()));
            {
                let mut r = ctx.report.borrow_mut();
                r.check("hover: state", st == State::Hover, format!("{st:?}"));
                r.check("hover: interactive", !ignores, "");
                r.check("hover: open animation ran", anim.frames >= 10, format!("{} frames", anim.frames));
                r.metrics.insert("open_animation".into(), anim_json(anim));
            }
            check_settled(ctx, "hover", &[LayoutKind::Compact]);
            let island = with_controller(|c| c.island_global()).unwrap_or_default();
            screenshot(ctx, mtm, "02-hover", Some(island.outset(20.0, 20.0, 20.0, 0.0)));
        }
        // ── click to pin ──
        4 => {
            let island = with_controller(|c| c.island_global()).unwrap_or_default();
            let p = NSPoint::new(island.mid_x(), island.y + island.height * 0.4);
            post_mouse(mtm, cg::kCGEventLeftMouseDown, p);
            post_mouse(mtm, cg::kCGEventLeftMouseUp, p);
        }
        5 => {
            let real = matches!(state(), Some(State::Expanded));
            ctx.report.borrow_mut().input("click", real);
            if !real {
                with_controller(|c| {
                    let island = c.island_global();
                    let win = c.panel_global();
                    c.root_mouse_down(NSPoint::new(island.mid_x() - win.x, island.y + island.height * 0.4 - win.y));
                });
            }
        }
        // ── drop two files on the tray ──
        6 => {
            let files_dir = ctx.dir.join("dropped");
            let _ = std::fs::create_dir_all(&files_dir);
            let files: Vec<PathBuf> = ["report.pdf", "screenshot.png"]
                .iter()
                .map(|n| {
                    let p = files_dir.join(n);
                    let _ = std::fs::write(&p, b"notchforge self-test");
                    p
                })
                .collect();
            with_controller(|c| {
                c.drag_entered();
                c.files_dropped(files);
            });
        }
        7 => {
            let (st, snap) = with_controller(|c| (c.machine_state(), c.current_snapshot())).unwrap_or_default_state();
            {
                let mut r = ctx.report.borrow_mut();
                r.check("expanded: state", st == State::Expanded, format!("{st:?}"));
                r.check("expanded: dropped files in tray", snap.tray.len() == 2, format!("{} items", snap.tray.len()));
                r.metrics.insert(
                    "controls".into(),
                    json!({
                        "brightness": crate::controls::brightness::get(),
                        "volume": crate::controls::volume::get(),
                        "display_modes": crate::controls::display_modes::active_displays()
                            .first()
                            .map(|d| crate::controls::display_modes::display_info(*d, String::new()).modes.len()),
                    }),
                );
            }
            check_settled(ctx, "expanded", &[LayoutKind::Expanded]);
            let island = with_controller(|c| c.island_global()).unwrap_or_default();
            screenshot(ctx, mtm, "03-expanded", Some(island.outset(20.0, 20.0, 20.0, 0.0)));
        }
        // ── Escape collapses ──
        8 => post_key(KEY_ESCAPE),
        9 => {
            let real = matches!(state(), Some(State::Collapsed));
            ctx.report.borrow_mut().input("escape", real);
            if !real {
                with_controller(|c| c.key_down(KEY_ESCAPE, NSEventModifierFlags::empty()));
            }
        }
        10 => {
            let (st, ignores, anim) = with_controller(|c| (c.machine_state(), c.ignores_mouse(), c.last_animation()))
                .unwrap_or((State::Hover, false, AnimStats::default()));
            {
                let mut r = ctx.report.borrow_mut();
                r.check("collapse: state", st == State::Collapsed, format!("{st:?}"));
                r.check("collapse: click-through again", ignores, "");
                r.check("collapse: animation ran", anim.frames >= 10, format!("{} frames", anim.frames));
                r.check(
                    "collapse: smooth (no frame > 50 ms)",
                    anim.max_frame_secs < 0.05,
                    format!("max frame {:.1} ms", anim.max_frame_secs * 1000.0),
                );
                r.metrics.insert("close_animation".into(), anim_json(anim));
            }
            check_settled(ctx, "collapse", &[LayoutKind::Collapsed, LayoutKind::CollapsedWithWings]);
            // Park the pointer well away from the notch: a pointer resting under
            // the notch would (correctly) turn the upcoming peek into a hover.
            post_mouse(mtm, cg::kCGEventMouseMoved, NSPoint::new(center.x, center.y - 400.0));
        }
        // ── a notification peeks out ──
        11 => app::send(Command::Inject(Box::new(NewNotification {
            app: AppRef::known(KnownApp::Claude),
            origin: Origin::Internal,
            kind: NotificationKind::NeedsInput,
            priority: Some(Priority::High),
            title: "Self-test · needs your input".into(),
            subtitle: None,
            body: "Claude needs your permission to use Bash".into(),
            timestamp_ms: notchforge_core::time::now_ms(),
            thread: None,
            url: None,
        }))),
        12 => {
            let st = state();
            ctx.report.borrow_mut().check("peek: state", st == Some(State::Peek), format!("{st:?}"));
            check_settled(ctx, "peek", &[LayoutKind::Peek]);
            let island = with_controller(|c| c.island_global()).unwrap_or_default();
            screenshot(ctx, mtm, "04-peek", Some(island.outset(40.0, 40.0, 30.0, 0.0)));
        }
        // ── settings window ──
        13 => app::show_settings(),
        14 => {
            let visible = app::settings_visible();
            ctx.report.borrow_mut().check("settings: window shown", visible, "");
            screenshot(ctx, mtm, "05-settings", None);
            app::close_settings();
        }
        // ── display reconfiguration keeps the UI consistent ──
        15 => {
            with_controller(|c| c.relayout());
            let perms: serde_json::Map<String, Value> = Permission::ALL
                .iter()
                .map(|p| (p.title().to_owned(), json!(crate::permissions::state_label(crate::permissions::state(*p)))))
                .collect();
            ctx.report.borrow_mut().metrics.insert("permissions".into(), Value::Object(perms));
            let alive = with_controller(|c| c.panel.isVisible()).unwrap_or(false);
            ctx.report.borrow_mut().check("relayout: panel still on screen", alive, "");
        }
        _ => finish(ctx),
    }
}

trait OrDefaultState {
    fn unwrap_or_default_state(self) -> (State, std::sync::Arc<notchforge_core::engine::Snapshot>);
}

impl OrDefaultState for Option<(State, std::sync::Arc<notchforge_core::engine::Snapshot>)> {
    fn unwrap_or_default_state(self) -> (State, std::sync::Arc<notchforge_core::engine::Snapshot>) {
        self.unwrap_or_else(|| (State::Collapsed, Default::default()))
    }
}

fn finish(ctx: &Rc<Ctx>) {
    let r = ctx.report.borrow();
    let passed = r.checks.iter().filter(|c| c["passed"] == json!(true)).count();
    let doc = json!({
        "result": if r.failed { "FAIL" } else { "PASS" },
        "passed": passed,
        "total": r.checks.len(),
        "checks": r.checks,
        "input": r.input,
        "metrics": r.metrics,
        "screenshots": r.screenshots,
    });
    let path = ctx.dir.join("selftest.json");
    let text = serde_json::to_string_pretty(&doc).unwrap_or_default();
    if let Err(e) = std::fs::write(&path, &text) {
        tracing::error!(error = %e, "could not write self-test report");
    }
    println!("{text}");
    tracing::info!(
        passed,
        total = r.checks.len(),
        result = if r.failed { "FAIL" } else { "PASS" },
        "self-test finished"
    );
    let code = i32::from(r.failed);
    drop(r);
    app::quit_with_code(code);
}

/// Schedule the self-test; steps run 800 ms apart (long enough for every
/// spring to settle at default speed).
pub fn start(mtm: MainThreadMarker, dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let ctx = Rc::new(Ctx { dir, report: RefCell::new(Report::default()) });
    let step = Rc::new(std::cell::Cell::new(0usize));
    let block = RcBlock::new(move |timer: NonNull<NSTimer>| {
        let n = step.get();
        step.set(n + 1);
        run_step(&ctx, mtm, n);
        if n >= 16 {
            // SAFETY: the timer passed to its own block is valid.
            unsafe { timer.as_ref() }.invalidate();
        }
    });
    // Give the app a moment to finish launching before the first step.
    // SAFETY: documented NSTimer API.
    let _timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.8, true, &block) };
}
