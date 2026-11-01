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
use notchforge_core::model::{NewNotification, NotificationKind, Origin, Permission, Priority, ProviderStatus};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEventModifierFlags, NSScreen};
use objc2_foundation::{NSPoint, NSTimer};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;

const KEY_ESCAPE: u16 = 53;

/// The island's own layer tree rendered to an RGBA bitmap (1 px per point).
/// Rendering our own layers needs no Screen Recording permission, unlike
/// `screencapture`, which only sees the wallpaper without it.
struct Raster {
    w: usize,
    h: usize,
    /// Premultiplied RGBA, first row = top of the island.
    px: Vec<u8>,
}

impl Raster {
    fn capture() -> Option<Raster> {
        use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGImageAlphaInfo};
        let (layer, island) = with_controller(|c| (c.island_layer(), c.island_global()))?;
        let layer = layer?;
        // The bitmap is sized to the *visible* island. The layer is taller (it
        // overhangs the window's top edge by one corner radius); in CoreGraphics'
        // y-up space that overhang lies above the bitmap and is clipped, exactly
        // as the window clips it on screen.
        let (w, h) = (island.width.round() as usize, island.height.round() as usize);
        if w == 0 || h == 0 {
            return None;
        }
        let mut px = vec![0u8; w * h * 4];
        let space = CGColorSpace::new_device_rgb()?;
        // SAFETY: `px` outlives the context (dropped at the end of the block)
        // and is exactly w*h*4 bytes with a w*4 stride.
        unsafe {
            let ctx = CGBitmapContextCreate(
                px.as_mut_ptr().cast(),
                w,
                h,
                8,
                w * 4,
                Some(&space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )?;
            layer.renderInContext(&ctx);
        }
        Some(Raster { w, h, px })
    }

    fn at(&self, x: usize, y: usize) -> (f64, f64) {
        let i = (y * self.w + x) * 4;
        let a = self.px[i + 3] as f64 / 255.0;
        let lum =
            (0.2126 * self.px[i] as f64 + 0.7152 * self.px[i + 1] as f64 + 0.0722 * self.px[i + 2] as f64) / 255.0;
        (a, lum)
    }

    /// (opaque fraction, fraction of bright "content" pixels)
    fn stats(&self) -> (f64, f64) {
        let (mut opaque, mut bright) = (0usize, 0usize);
        for y in 0..self.h {
            for x in 0..self.w {
                let (a, l) = self.at(x, y);
                if a > 0.8 {
                    opaque += 1;
                    if l > 0.35 {
                        bright += 1;
                    }
                }
            }
        }
        let n = (self.w * self.h) as f64;
        (opaque as f64 / n, bright as f64 / n)
    }

    /// Coarse text rendering: ' ' transparent, '.' black island, ':' 'o' '@' brighter content.
    fn ascii(&self, cols: usize) -> Vec<String> {
        let cols = cols.min(self.w).max(1);
        let cw = self.w as f64 / cols as f64;
        let rows = ((self.h as f64 / cw) * 0.5).round().max(1.0) as usize;
        let ch = self.h as f64 / rows as f64;
        (0..rows)
            .map(|r| {
                (0..cols)
                    .map(|c| {
                        let (x0, y0) = ((c as f64 * cw) as usize, (r as f64 * ch) as usize);
                        let (x1, y1) = (((c + 1) as f64 * cw) as usize, ((r + 1) as f64 * ch) as usize);
                        let (mut a, mut l, mut n) = (0.0, 0.0, 0.0);
                        for y in y0..y1.min(self.h) {
                            for x in x0..x1.min(self.w) {
                                let (pa, pl) = self.at(x, y);
                                a += pa;
                                l += pl;
                                n += 1.0;
                            }
                        }
                        let (a, l) = if n > 0.0 { (a / n, l / n) } else { (0.0, 0.0) };
                        match (a, l) {
                            (a, _) if a < 0.4 => ' ',
                            (_, l) if l < 0.06 => '.',
                            (_, l) if l < 0.18 => ':',
                            (_, l) if l < 0.4 => 'o',
                            _ => '@',
                        }
                    })
                    .collect()
            })
            .collect()
    }
}

/// Pixel-level checks on the rendered island plus an ASCII preview in the log.
fn check_pixels(ctx: &Ctx, stage: &str, expect_content: bool, rounded_bottom: bool) {
    let Some(r) = Raster::capture() else {
        ctx.report.borrow_mut().check(&format!("{stage}: island renders"), false, "could not render layer");
        return;
    };
    let (opaque, bright) = r.stats();
    let preview = r.ascii(88);
    println!("--- {stage} ({}×{} pt) ---", r.w, r.h);
    for line in &preview {
        println!("|{line}|");
    }
    let corner = |x: usize, y: usize| r.at(x, y).0;
    let (tl, bl) = (corner(0, 0), corner(0, r.h - 1));
    let mut rep = ctx.report.borrow_mut();
    rep.check(
        &format!("{stage}: island is an opaque black shape"),
        opaque > 0.85,
        format!("{:.1}% opaque", opaque * 100.0),
    );
    if expect_content {
        rep.check(
            &format!("{stage}: content is drawn"),
            bright > 0.004,
            format!("{:.2}% bright pixels", bright * 100.0),
        );
    }
    rep.check(&format!("{stage}: top edge is square (merges with bezel)"), tl > 0.8, format!("top-left alpha {tl:.2}"));
    if rounded_bottom {
        rep.check(&format!("{stage}: bottom corners are rounded"), bl < 0.5, format!("bottom-left alpha {bl:.2}"));
    }
    rep.metrics.insert(format!("render_{stage}"), json!({ "w": r.w, "h": r.h, "opaque": opaque, "bright": bright }));
}

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
/// Save a PNG and print it base64-encoded (chunked) so CI logs carry the
/// real rendering for review (`NFPNG <name> <index> <chunk>` lines).
fn emit_png(ctx: &Ctx, name: &str, png: Vec<u8>) {
    use base64::Engine as _;
    let path = ctx.dir.join(format!("{name}.png"));
    if std::fs::write(&path, &png).is_ok() {
        ctx.report.borrow_mut().screenshots.push(path.display().to_string());
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    for (i, chunk) in b64.as_bytes().chunks(12_000).enumerate() {
        println!("NFPNG {name} {i} {}", String::from_utf8_lossy(chunk));
    }
}

/// PNG of a layer tree rendered offscreen at `scale` (no Screen Recording
/// permission needed: these are our own layers).
fn png_from_layer(layer: &objc2_quartz_core::CALayer, w: f64, h: f64, scale: f64) -> Option<Vec<u8>> {
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGContext, CGImageAlphaInfo,
    };
    let (pw, ph) = ((w * scale).round() as usize, (h * scale).round() as usize);
    if pw == 0 || ph == 0 {
        return None;
    }
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: null data lets CoreGraphics own the buffer; 0 = automatic stride.
    let ctx = unsafe {
        CGBitmapContextCreate(std::ptr::null_mut(), pw, ph, 8, 0, Some(&space), CGImageAlphaInfo::PremultipliedLast.0)
    }?;
    CGContext::scale_ctm(Some(&ctx), scale, scale);
    layer.renderInContext(&ctx);
    let img = CGBitmapContextCreateImage(Some(&ctx))?;
    png_from_cgimage(&img)
}

fn png_from_cgimage(img: &objc2_core_graphics::CGImage) -> Option<Vec<u8>> {
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), img);
    // SAFETY: empty properties dictionary.
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &objc2_foundation::NSDictionary::new())
    }?;
    Some(data.to_vec())
}

/// PNG of any view, drawn offscreen (used for the Settings window).
pub fn png_from_view(view: &objc2_app_kit::NSView) -> Option<Vec<u8>> {
    use objc2_app_kit::NSBitmapImageFileType;
    let bounds = view.bounds();
    let rep = view.bitmapImageRepForCachingDisplayInRect(bounds)?;
    view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    // SAFETY: empty properties dictionary.
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &objc2_foundation::NSDictionary::new())
    }?;
    Some(data.to_vec())
}

/// Render the island as it is on screen right now, at 2×.
fn screenshot(ctx: &Ctx, _mtm: MainThreadMarker, name: &str, _region: Option<Rect>) {
    // Draw pending view content (scroll views draw lazily) and commit layers.
    with_controller(|c| c.panel.displayIfNeeded());
    objc2_quartz_core::CATransaction::flush();
    let Some((layer, island)) = with_controller(|c| (c.island_layer(), c.island_global())) else { return };
    let Some(layer) = layer else { return };
    match png_from_layer(&layer, island.width, island.height, 2.0) {
        Some(png) => emit_png(ctx, name, png),
        None => tracing::warn!(name, "could not render the island"),
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
            check_pixels(ctx, "hover", true, true);
            let island = with_controller(|c| c.island_global()).unwrap_or_default();
            screenshot(ctx, mtm, "02-hover", Some(island.outset(20.0, 20.0, 20.0, 0.0)));
            // The same glance with a playing track, as on the design board.
            if let Some(real) = with_controller(|c| c.current_snapshot()) {
                let mut fake = (*real).clone();
                fake.media = sample_media();
                fake.unread = 0;
                with_controller(|c| c.preview_compact(&fake));
                screenshot(ctx, mtm, "02b-hover-music", None);
                with_controller(|c| c.preview_compact(&real));
            }
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
            check_pixels(ctx, "expanded", true, true);
            let island = with_controller(|c| c.island_global()).unwrap_or_default();
            screenshot(ctx, mtm, "03-expanded", Some(island.outset(20.0, 20.0, 20.0, 0.0)));
            music_page(ctx, mtm, &snap);
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
            check_pixels(ctx, "collapsed-wings", true, false);
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
            check_pixels(ctx, "peek", true, true);
            let island = with_controller(|c| c.island_global()).unwrap_or_default();
            screenshot(ctx, mtm, "04-peek", Some(island.outset(40.0, 40.0, 30.0, 0.0)));
        }
        // ── settings window ──
        13 => app::show_settings_pane(crate::settings::Pane::Media),
        14 => {
            let visible = app::settings_visible();
            ctx.report.borrow_mut().check("settings: window shown", visible, "");
            match app::settings_content_view().as_deref().and_then(png_from_view) {
                Some(png) => emit_png(ctx, "05-settings", png),
                None => tracing::warn!("could not render the Settings window"),
            }
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

/// Music page: live audio metrics, then a synthetic Spotify session rendered
/// through the real view code (title, playlists, pixels).
fn music_page(ctx: &Rc<Ctx>, mtm: MainThreadMarker, snap: &notchforge_core::engine::Snapshot) {
    use crate::views::expanded::Tab;
    with_controller(|c| c.show_expanded_tab(Tab::Media));
    let shown = with_controller(|c| c.media_page_visible()).unwrap_or(false);
    {
        let mut r = ctx.report.borrow_mut();
        r.check("music page: shown", shown, "");
        let audio_ok = snap.providers.get("audio").is_some_and(|s| s.is_healthy());
        r.check("audio provider: healthy", audio_ok, format!("{:?}", snap.providers.get("audio")));
        let media_ok = snap
            .providers
            .get("media")
            .is_some_and(|s| s.is_healthy() || matches!(s, ProviderStatus::NeedsPermission(Permission::Automation)));
        r.check("media provider: healthy", media_ok, format!("{:?}", snap.providers.get("media")));
        r.metrics.insert(
            "audio".into(),
            json!({
                "outputs": snap.audio.devices.iter().map(|d| json!({
                    "name": d.name, "capabilities": d.capabilities(), "default": d.is_default,
                })).collect::<Vec<_>>(),
                "per_app_supported": snap.audio.sources_supported,
                "playing": snap.audio.sources.iter().map(|s| format!("{} -> {}", s.name, snap.audio.route_label(s))).collect::<Vec<_>>(),
                "players_running": snap.media.running.iter().map(|p| p.display_name()).collect::<Vec<_>>(),
                "now_playing": snap.media.primary().map(|n| format!("{} - {}", n.title, n.artist)),
            }),
        );
    }

    let mut fake = snap.clone();
    fake.generation = u64::MAX - 1;
    fake.media = sample_media();
    let (title, rows) = with_controller(|c| c.preview_media(&fake)).unwrap_or_default();
    {
        let mut r = ctx.report.borrow_mut();
        r.check("music page: renders track", title == "Self-test track", title);
        r.check("music page: lists playlists", rows == 3, format!("{rows} rows"));
    }
    check_pixels(ctx, "music page", true, true);
    if let Some(png) = app::menubar_preview(&fake).as_deref().and_then(png_from_view) {
        emit_png(ctx, "06-menubar-player", png);
    }
    let island = with_controller(|c| c.island_global()).unwrap_or_default();
    screenshot(ctx, mtm, "03b-music", Some(island.outset(20.0, 20.0, 20.0, 0.0)));
    // Back to the real data and the overview page.
    with_controller(|c| {
        c.media_rerender();
        c.show_expanded_tab(Tab::Overview);
    });
    overview_preview(ctx, mtm, snap);
}

/// The Overview with design-like sample data (CI hosts have no weather,
/// accessories or agents), rendered for review, then the real data again.
fn overview_preview(ctx: &Rc<Ctx>, mtm: MainThreadMarker, snap: &notchforge_core::engine::Snapshot) {
    use notchforge_core::model::{
        BatteryComponent, BatteryStatus, DeviceBattery, DeviceKind, HourlyForecast, Notification, NotificationId,
        TemperatureUnit, Weather, WeatherCondition,
    };
    let now = notchforge_core::time::now_ms();
    let mut fake = snap.clone();
    fake.generation = u64::MAX - 2;
    fake.battery = Some(BatteryStatus {
        percent: 82,
        charging: false,
        plugged_in: false,
        minutes_remaining: Some(312),
        present: true,
    });
    fake.weather = Some(Weather {
        place: "Paris".into(),
        temperature: 10.0,
        feels_like: 8.0,
        condition: WeatherCondition::PartlyCloudy,
        is_day: true,
        high: 14.0,
        low: 7.0,
        unit: TemperatureUnit::Celsius,
        hourly: [
            (22, 9.0, WeatherCondition::Cloudy),
            (23, 8.0, WeatherCondition::Rain),
            (0, 8.0, WeatherCondition::Rain),
            (1, 7.0, WeatherCondition::Clear),
        ]
        .iter()
        .map(|(h, t, c)| HourlyForecast {
            hour: format!("{h:02}"),
            temperature: *t,
            condition: *c,
            precipitation_chance: None,
        })
        .collect(),
        fetched_at_ms: now,
    });
    fake.devices = vec![
        DeviceBattery {
            id: "a".into(),
            name: "AirPods Pro".into(),
            kind: DeviceKind::AirPods,
            levels: vec![(BatteryComponent::Left, 80), (BatteryComponent::Right, 78), (BatteryComponent::Case, 18)],
        },
        DeviceBattery {
            id: "m".into(),
            name: "Magic Mouse".into(),
            kind: DeviceKind::Mouse,
            levels: vec![(BatteryComponent::Main, 64)],
        },
    ];
    let note = |id: u64, app: KnownApp, kind: NotificationKind, prio: Priority, title: &str, body: &str, ago: i64| {
        Notification {
            id: NotificationId(id),
            app: AppRef::known(app),
            origin: Origin::Internal,
            kind,
            priority: prio,
            title: title.into(),
            subtitle: None,
            body: body.into(),
            timestamp_ms: now - ago,
            thread: None,
            url: None,
            group_count: 1,
            read: id > 1,
        }
    };
    fake.notifications = vec![
        note(
            1,
            KnownApp::Claude,
            NotificationKind::NeedsInput,
            Priority::High,
            "Allow running cargo test?",
            "notchforge · waiting for your approval",
            5_000,
        ),
        note(
            2,
            KnownApp::Cursor,
            NotificationKind::TaskFinished,
            Priority::Normal,
            "Settings refactor complete",
            "4 files changed, all tests pass",
            360_000,
        ),
        note(
            3,
            KnownApp::Messages,
            NotificationKind::Message,
            Priority::Normal,
            "Léa",
            "Dinner at 8? I booked the place on Rue Oberkampf",
            720_000,
        ),
    ];
    fake.unread = 1;
    with_controller(|c| c.preview_overview(&fake));
    check_pixels(ctx, "overview sample", true, true);
    screenshot(ctx, mtm, "03c-overview-sample", None);
    with_controller(|c| c.preview_overview(snap));
    notched_previews(ctx, &fake);
    text_probe(ctx, mtm);
}

/// The Peek title drawn three ways, to tell how much of the text weight seen
/// in the renders comes from font smoothing in the capture: through an
/// NSTextField (the app's path), and straight into a bitmap with smoothing
/// on and off.
fn text_probe(ctx: &Rc<Ctx>, mtm: MainThreadMarker) {
    use objc2::MainThreadOnly;
    use objc2_app_kit::{NSAttributedStringNSStringDrawing, NSGraphicsContext};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGContext, CGImageAlphaInfo,
    };
    let (w, h, scale) = (200.0, 24.0, 2.0);
    let text = "Allow running cargo test?";
    let font = crate::ui::font(14.0, crate::ui::Weight::Semibold);
    let white = objc2_app_kit::NSColor::whiteColor();
    for smooth in [true, false] {
        let (pw, ph) = ((w * scale) as usize, (h * scale) as usize);
        let Some(space) = CGColorSpace::new_device_rgb() else { return };
        // SAFETY: null data lets CoreGraphics own the buffer; 0 = automatic stride.
        let Some(cg) = (unsafe {
            CGBitmapContextCreate(
                std::ptr::null_mut(),
                pw,
                ph,
                8,
                0,
                Some(&space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }) else {
            return;
        };
        CGContext::set_rgb_fill_color(Some(&cg), 0.0, 0.0, 0.0, 1.0);
        CGContext::fill_rect(Some(&cg), crate::ui::rect(0.0, 0.0, pw as f64, ph as f64));
        CGContext::scale_ctm(Some(&cg), scale, scale);
        CGContext::set_allows_font_smoothing(Some(&cg), smooth);
        CGContext::set_should_smooth_fonts(Some(&cg), smooth);
        let ns = NSGraphicsContext::graphicsContextWithCGContext_flipped(&cg, false);
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&ns));
        crate::ui::attributed(text, &font, &white).drawAtPoint(NSPoint::new(0.0, 5.0));
        NSGraphicsContext::restoreGraphicsState_class();
        let Some(img) = CGBitmapContextCreateImage(Some(&cg)) else { return };
        if let Some(png) = png_from_cgimage(&img) {
            let name = if smooth { "08-text-smoothing-on" } else { "08-text-smoothing-off" };
            emit_png(ctx, name, png);
        }
    }
    // The app's path: a label in a layer-backed view, captured like the islands.
    let host = crate::ui::FlippedView::new(mtm, crate::ui::rect(0.0, 0.0, w, h));
    host.setWantsLayer(true);
    if let Some(l) = host.layer() {
        l.setBackgroundColor(Some(&objc2_app_kit::NSColor::blackColor().CGColor()));
    }
    host.addSubview(&crate::ui::label(mtm, text, &font, &white, crate::ui::rect(0.0, 2.0, w, 20.0)));
    // SAFETY: a plain borderless window, closed below.
    let window = unsafe {
        let win = objc2_app_kit::NSWindow::initWithContentRect_styleMask_backing_defer(
            objc2_app_kit::NSWindow::alloc(mtm),
            crate::ui::rect(-20000.0, -20000.0, w, h),
            objc2_app_kit::NSWindowStyleMask::Borderless,
            objc2_app_kit::NSBackingStoreType::Buffered,
            false,
        );
        win.setReleasedWhenClosed(false);
        win
    };
    window.setContentView(Some(&host));
    window.displayIfNeeded();
    objc2_quartz_core::CATransaction::flush();
    if let Some(png) = host.layer().and_then(|l| png_from_layer(&l, w, h, scale)) {
        emit_png(ctx, "08-text-label", png);
    }
    window.close();
}

/// The same sample data on a simulated 14-inch MacBook Pro notch (185 × 32
/// pt), the geometry the design boards are drawn at. CI Macs have no notch
/// and a 25 pt menu bar, so the live screenshots above sit 7 pt shorter.
fn notched_previews(ctx: &Rc<Ctx>, sample: &notchforge_core::engine::Snapshot) {
    let (overview, music) = board_snapshots(ctx, sample);
    let Some(peek) = overview.notifications.first().cloned() else { return };
    let (overview, music) = (&overview, &music);
    let mut sizes = Vec::new();
    with_controller(|c| {
        c.notched_previews(overview, music, &peek, &mut |name, view, w, h| {
            sizes.push((name.to_string(), w, h));
            match view.layer().and_then(|l| png_from_layer(&l, w, h, 2.0)) {
                Some(png) => emit_png(ctx, name, png),
                None => tracing::warn!(name, "could not render the notched preview"),
            }
        })
    });
    let mut r = ctx.report.borrow_mut();
    let size = |n: &str| sizes.iter().find(|s| s.0 == n).map(|s| (s.1, s.2));
    r.check("notched: four views rendered", sizes.len() == 4, format!("{sizes:?}"));
    r.metrics.insert("notched_sizes".into(), json!(sizes));
    // Island sizes on the design boards (Overview, Main, Hover, Peek).
    for (name, bw, bh) in [
        ("07a-notched-overview", 720.0, 372.0),
        ("07b-notched-music", 720.0, 372.0),
        ("07c-notched-hover", 520.0, 176.0),
        ("07d-notched-peek", 440.0, 96.0),
    ] {
        let got = size(name);
        r.check(&format!("notched: {name} is {bw} × {bh} like its board"), got == Some((bw, bh)), format!("{got:?}"));
    }
}

/// The exact content of the design boards (Overview, Now Playing, Hover),
/// so the notched renders differ from the boards only where the UI does.
fn board_snapshots(
    ctx: &Ctx,
    sample: &notchforge_core::engine::Snapshot,
) -> (notchforge_core::engine::Snapshot, notchforge_core::engine::Snapshot) {
    use notchforge_core::model::{
        AudioDevice, AudioSource, AudioState, AudioTransport, FocusState, MediaPlayer, MediaState, Notification,
        NotificationId, NowPlaying, PlaybackState, Playlist, PlaylistStatus, TrayItem, TrayItemId,
    };
    let now = notchforge_core::time::now_ms();
    let dir = ctx.dir.join("board");
    let _ = std::fs::create_dir_all(dir.join("Assets"));

    // Overview: four notifications, three tray items, Focus on.
    let mut overview = sample.clone();
    overview.generation = u64::MAX - 4;
    let mut claude_code = AppRef::known(KnownApp::Claude);
    claude_code.display_name = "Claude Code".into();
    let note = |id: u64, app: AppRef, kind: NotificationKind, prio: Priority, title: &str, body: &str, ago: i64| {
        Notification {
            id: NotificationId(id),
            app,
            origin: Origin::Internal,
            kind,
            priority: prio,
            title: title.into(),
            subtitle: None,
            body: body.into(),
            timestamp_ms: now - ago,
            thread: None,
            url: None,
            group_count: 1,
            read: true,
        }
    };
    overview.notifications = vec![
        note(
            11,
            claude_code,
            NotificationKind::NeedsInput,
            Priority::High,
            "Allow running cargo test?",
            "notchforge · waiting for your approval",
            5_000,
        ),
        note(
            12,
            AppRef::known(KnownApp::Cursor),
            NotificationKind::TaskFinished,
            Priority::Normal,
            "Settings refactor complete",
            "4 files changed, all tests pass",
            360_000,
        ),
        note(
            13,
            AppRef::known(KnownApp::Messages),
            NotificationKind::Message,
            Priority::Normal,
            "Léa",
            "Dinner at 8? I booked the place on Rue Oberkampf",
            720_000,
        ),
        note(14, AppRef::known(KnownApp::Slack), NotificationKind::Message, Priority::Normal, "Mira", "", 1_500_000),
    ];
    overview.unread = 1;
    let report = dir.join("Report.pdf");
    let _ = std::fs::write(&report, b"%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n");
    let mockup = dir.join("Mockup.png");
    if let Some(png) =
        art_png(220.0, 220.0, &[(0.0, (0.36, 0.66, 1.0)), (1.0, (0.18, 0.31, 0.85))], (0.2, 0.0), &[], None)
    {
        let _ = std::fs::write(&mockup, png);
    }
    let item = |id: u64, path: std::path::PathBuf, is_dir: bool| TrayItem {
        id: TrayItemId(id),
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        path,
        is_dir,
        bytes: None,
        added_at_ms: now,
    };
    overview.tray = vec![item(1, report, false), item(2, mockup, false), item(3, dir.join("Assets"), true)];
    overview.focus = FocusState { active: Some(true), mode_name: Some("Do Not Disturb".into()) };

    // Now Playing: Afterglow on AirPods Pro, Spotify and Safari playing, five playlists.
    let mut music = overview.clone();
    music.generation = u64::MAX - 5;
    music.notifications.clear();
    music.unread = 0;
    let airpods = "board-airpods".to_string();
    music.audio = AudioState {
        devices: vec![AudioDevice {
            id: 1,
            uid: airpods.clone(),
            name: "AirPods Pro".into(),
            transport: AudioTransport::Bluetooth,
            sample_rate: Some(48_000.0),
            max_sample_rate: Some(48_000.0),
            bit_depth: Some(24),
            output_channels: 2,
            volume: Some(0.62),
            muted: Some(false),
            is_default: true,
        }],
        sources: [("Spotify", "com.spotify.client"), ("Safari", "com.apple.Safari")]
            .iter()
            .enumerate()
            .map(|(i, (name, bundle))| AudioSource {
                pid: 100 + i as i32,
                name: (*name).into(),
                bundle_id: Some((*bundle).into()),
                app_path: None,
                device_uids: vec![airpods.clone()],
            })
            .collect(),
        sources_supported: true,
    };
    let cover = dir.join("afterglow.png");
    if let Some(png) = art_png(
        272.0,
        272.0,
        &[(0.0, (0.357, 0.114, 0.420)), (0.72, (0.114, 0.059, 0.227)), (1.0, (0.114, 0.059, 0.227))],
        (0.15, 0.0),
        &[((0.30, 0.28), 0.52, (1.0, 0.365, 0.561)), ((0.70, 0.78), 0.36, (1.0, 0.702, 0.420))],
        Some("NIGHT TRANSIT"),
    ) {
        let _ = std::fs::write(&cover, png);
    }
    /// Name, tracks, owner and the three gradient stops of its cover.
    type BoardPlaylist<'a> = (&'a str, u32, &'a str, [(f64, f64, f64); 3]);
    let lists: [BoardPlaylist; 5] = [
        ("Night Drive", 64, "You", [(1.0, 0.365, 0.561), (0.357, 0.114, 0.420), (0.078, 0.043, 0.18)]),
        ("Deep Focus", 120, "You", [(0.243, 0.78, 0.761), (0.106, 0.302, 0.42), (0.043, 0.102, 0.165)]),
        ("Sunday Slow", 38, "Léa", [(1.0, 0.824, 0.478), (0.878, 0.478, 0.247), (0.353, 0.141, 0.071)]),
        ("Run Club", 45, "You", [(0.71, 1.0, 0.42), (0.184, 0.561, 0.306), (0.051, 0.165, 0.094)]),
        ("Rainy Paris", 52, "You", [(0.616, 0.706, 1.0), (0.29, 0.333, 0.788), (0.086, 0.102, 0.267)]),
    ];
    let playlists = lists
        .iter()
        .enumerate()
        .map(|(i, (name, count, owner, c))| {
            let path = dir.join(format!("playlist-{i}.png"));
            if let Some(png) = art_png(96.0, 96.0, &[(0.0, c[0]), (0.58, c[1]), (1.0, c[2])], (0.0, 0.0), &[], None) {
                let _ = std::fs::write(&path, png);
            }
            Playlist {
                player: MediaPlayer::Spotify,
                id: format!("spotify:playlist:board{i}"),
                name: (*name).into(),
                track_count: Some(*count),
                owner: Some((*owner).into()),
                artwork_url: None,
                artwork_path: Some(path),
            }
        })
        .collect();
    music.media = MediaState {
        players: vec![NowPlaying {
            player: MediaPlayer::Spotify,
            state: PlaybackState::Playing,
            title: "Afterglow".into(),
            artist: "Neon Harbor".into(),
            album: "Night Transit".into(),
            duration: Some(221.0),
            position: Some(84.0),
            sampled_at_ms: now,
            track_id: Some("spotify:track:afterglow".into()),
            artwork_path: Some(cover),
            shuffle: Some(true),
            repeat: Some(false),
            volume: Some(62),
        }],
        running: vec![MediaPlayer::Spotify],
        playlists,
        playlist_status: vec![(MediaPlayer::Spotify, PlaylistStatus::Loaded)],
        automation_denied: vec![],
    };
    (overview, music)
}

/// A square of cover art like the boards' CSS: an axial gradient from
/// `start` (unit point, top-left origin) to the opposite corner, radial
/// glows (centre, radius, colour) and an optional letter-spaced caption.
#[allow(clippy::type_complexity)]
fn art_png(
    w: f64,
    h: f64,
    stops: &[(f64, (f64, f64, f64))],
    start: (f64, f64),
    glows: &[((f64, f64), f64, (f64, f64, f64))],
    caption: Option<&str>,
) -> Option<Vec<u8>> {
    use objc2_foundation::{NSNumber, NSString};
    use objc2_quartz_core::{CAGradientLayer, CALayer, CATextLayer};
    let cg = |r: f64, g: f64, b: f64, a: f64| crate::ui::rgba(r, g, b, a).CGColor();
    let root = CALayer::layer();
    root.setFrame(crate::ui::rect(0.0, 0.0, w, h));
    // Layer space is y-up; the boards' CSS is y-down.
    let base = CAGradientLayer::layer();
    base.setFrame(root.bounds());
    base.setStartPoint(NSPoint::new(start.0, 1.0 - start.1));
    base.setEndPoint(NSPoint::new(1.0 - start.0, start.1));
    crate::ui::set_gradient(&base, &stops.iter().map(|(_, (r, g, b))| (*r, *g, *b, 1.0)).collect::<Vec<_>>());
    let locs: Vec<_> = stops.iter().map(|(l, _)| NSNumber::new_f64(*l)).collect();
    base.setLocations(Some(&objc2_foundation::NSArray::from_retained_slice(&locs)));
    root.addSublayer(&base);
    for ((cx, cy), r, (cr, cgc, cb)) in glows {
        let g = CAGradientLayer::layer();
        g.setType(&NSString::from_str("radial"));
        g.setFrame(root.bounds());
        g.setStartPoint(NSPoint::new(*cx, 1.0 - cy));
        g.setEndPoint(NSPoint::new(cx + r, 1.0 - cy + r));
        crate::ui::set_gradient(&g, &[(*cr, *cgc, *cb, 1.0), (*cr, *cgc, *cb, 0.0)]);
        root.addSublayer(&g);
    }
    if let Some(text) = caption {
        // Two thin rings and a spaced caption, as on the Now Playing board.
        for (d, a) in [(0.68, 0.22), (0.36, 0.18)] {
            let ring = CALayer::layer();
            let side = w * d;
            ring.setFrame(crate::ui::rect((w - side) / 2.0, h * 0.5 - side / 2.0 + h * 0.03, side, side));
            ring.setCornerRadius(side / 2.0);
            ring.setBorderWidth(w / 136.0);
            ring.setBorderColor(Some(&cg(1.0, 1.0, 1.0, a)));
            root.addSublayer(&ring);
        }
        let t = CATextLayer::layer();
        t.setFrame(crate::ui::rect(0.0, h * 0.10, w, h * 0.09));
        let spaced: String = text.chars().map(|c| c.to_string()).collect::<Vec<_>>().join("\u{200A}\u{2009}");
        let font = crate::ui::font(h * 0.066, crate::ui::Weight::Semibold);
        let string = crate::ui::attributed(&spaced, &font, &crate::ui::rgba(1.0, 1.0, 1.0, 0.85));
        let string: &objc2::runtime::AnyObject = &string;
        // SAFETY: CATextLayer takes an NSAttributedString as its string.
        unsafe {
            t.setString(Some(string));
            t.setAlignmentMode(objc2_quartz_core::kCAAlignmentCenter);
        }
        root.addSublayer(&t);
    }
    png_from_layer(&root, w, h, 1.0)
}

/// A playing Spotify track with three playlists (self-test renders).
fn sample_media() -> notchforge_core::model::MediaState {
    use notchforge_core::model::{MediaPlayer, MediaState, NowPlaying, PlaybackState, Playlist, PlaylistStatus};
    MediaState {
        players: vec![NowPlaying {
            player: MediaPlayer::Spotify,
            state: PlaybackState::Playing,
            title: "Self-test track".into(),
            artist: "NotchForge".into(),
            album: "Diagnostics".into(),
            duration: Some(215.0),
            position: Some(42.0),
            sampled_at_ms: notchforge_core::time::now_ms(),
            track_id: Some("spotify:track:selftest".into()),
            artwork_path: None,
            shuffle: Some(true),
            repeat: Some(false),
            volume: Some(70),
        }],
        running: vec![MediaPlayer::Spotify],
        playlists: (1..=3)
            .map(|i| Playlist {
                player: MediaPlayer::Spotify,
                id: format!("spotify:playlist:{i}"),
                name: format!("Self-test playlist {i}"),
                track_count: Some(10 * i),
                owner: Some("NotchForge".into()),
                artwork_url: None,
                artwork_path: None,
            })
            .collect(),
        playlist_status: vec![(MediaPlayer::Spotify, PlaylistStatus::Loaded)],
        automation_denied: vec![],
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
