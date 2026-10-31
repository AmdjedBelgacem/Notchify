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
        13 => app::show_settings(),
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
    use notchforge_core::model::{MediaPlayer, MediaState, NowPlaying, PlaybackState, Playlist, PlaylistStatus};
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
    fake.media = MediaState {
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
    };
    let (title, rows) = with_controller(|c| c.preview_media(&fake)).unwrap_or_default();
    {
        let mut r = ctx.report.borrow_mut();
        r.check("music page: renders track", title == "Self-test track", title);
        r.check("music page: lists playlists", rows == 3, format!("{rows} rows"));
    }
    check_pixels(ctx, "music page", true, true);
    let island = with_controller(|c| c.island_global()).unwrap_or_default();
    screenshot(ctx, mtm, "03b-music", Some(island.outset(20.0, 20.0, 20.0, 0.0)));
    // Back to the real data and the overview page.
    with_controller(|c| {
        c.media_rerender();
        c.show_expanded_tab(Tab::Overview);
    });
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
