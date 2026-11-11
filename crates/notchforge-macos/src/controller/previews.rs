//! Rendering views from arbitrary snapshots, for the self-test and the
//! design previews.

use super::*;

impl Controller {
    /// Render the Overview from an arbitrary snapshot (self-test).
    pub fn preview_overview(&self, snap: &Snapshot) {
        self.views.borrow().expanded.update(snap);
    }

    /// Build every view for a simulated 14-inch MacBook Pro (1512 × 982,
    /// 185 × 32 pt notch: the geometry the design boards use), draw them from
    /// the given snapshots and hand each island, hosted in an offscreen
    /// window, to `render` (self-test).
    pub fn notched_previews(
        &self,
        overview: &Snapshot,
        music: &Snapshot,
        peek: &Notification,
        prompts: &[Notification; 2],
        render: &mut dyn FnMut(&str, &NSView, f64, f64),
    ) {
        use crate::views::expanded::Tab;
        use notchforge_core::geometry::{Rect, ScreenInfo};
        use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowStyleMask};
        let screen = ScreenInfo {
            id: 0,
            frame: Rect::new(0.0, 0.0, 1512.0, 982.0),
            visible_frame: Rect::new(0.0, 0.0, 1512.0, 950.0),
            safe_area_top: 32.0,
            aux_top_left: Some(Rect::new(0.0, 950.0, 663.5, 32.0)),
            aux_top_right: Some(Rect::new(848.5, 950.0, 663.5, 32.0)),
            backing_scale: 2.0,
            is_builtin: true,
            is_main: false,
        };
        let cfg = self.config.borrow().clone();
        let layout = NotchLayout::new(screen, cfg.appearance.clone());
        let views = Views::new(self.mtm, &layout, &cfg, &self.engine);
        let palette = self.palette.borrow().clone();

        let mut show = |name: &str, kind: LayoutKind, content: &NSView| {
            let s = layout.shape(kind);
            // SAFETY: a plain borderless window; it is closed and dropped
            // below and nothing else holds it, so it must not self-release.
            let window = unsafe {
                let w = NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(self.mtm),
                    rect(-20000.0, -20000.0, s.width, s.height),
                    NSWindowStyleMask::Borderless,
                    NSBackingStoreType::Buffered,
                    false,
                );
                w.setReleasedWhenClosed(false);
                w
            };
            let island = NSView::initWithFrame(NSView::alloc(self.mtm), rect(0.0, 0.0, s.width, s.height));
            island.setWantsLayer(true);
            if let Some(l) = island.layer() {
                l.setMasksToBounds(true);
                // On screen the top corners sit above the display edge; only
                // the lower two are visible.
                l.setCornerRadius(s.corner_radius);
                l.setMaskedCorners(
                    objc2_quartz_core::CACornerMask::LayerMinXMinYCorner
                        | objc2_quartz_core::CACornerMask::LayerMaxXMinYCorner,
                );
            }
            let notch_h = self.layout.borrow().notch.rect.height;
            let t = if matches!(kind, LayoutKind::Collapsed | LayoutKind::CollapsedWithWings(_)) { 0.0 } else { 1.0 };
            let _material =
                crate::glass::Glass::preview(self.mtm, &island, s.width, s.height, s.corner_radius, notch_h, t);
            content.setHidden(false);
            // Top-aligned, as on screen: a view can be taller than this shape.
            content.setFrameOrigin(NSPoint::new(0.0, s.height - content.frame().size.height));
            island.addSubview(content);
            window.setContentView(Some(&island));
            // On screen (far outside every display, never key) so layer-backed
            // views are drawn the way they are in the real panel.
            window.orderFrontRegardless();
            window.displayIfNeeded();
            objc2_quartz_core::CATransaction::flush();
            render(name, &island, s.width, s.height);
            content.removeFromSuperview();
            window.close();
        };

        views.expanded.update(overview);
        views.expanded.show_tab(Tab::Overview);
        show("07a-notched-overview", LayoutKind::Expanded, &views.expanded.root);
        views.expanded.media.invalidate();
        views.expanded.media.set_active_playlist(music.media.playlists.first().map(|p| p.id.clone()));
        views.expanded.media.update(music);
        views.expanded.show_tab(Tab::Media);
        show("07b-notched-music", LayoutKind::Expanded, &views.expanded.root);
        views.peek.show(peek, &palette);
        show("07d-notched-peek", LayoutKind::Peek, &views.peek.root);
        // Collapsed with wings while a browser tab plays audio (no Spotify / Music).
        let mut browser = music.clone();
        browser.media.players.clear();
        browser.audio.sources = vec![notchforge_core::model::AudioSource {
            pid: 200,
            name: "Google Chrome".into(),
            bundle_id: Some("com.google.Chrome".into()),
            app_path: None,
            device_uids: vec![],
        }];
        views.collapsed.update(
            &browser,
            &palette,
            crate::views::collapsed::WingOptions {
                media: true,
                animate: false,
                mask: WING_LEFT | WING_RIGHT,
                chevrons: 0,
            },
        );
        show(
            "07e-notched-wings-browser",
            LayoutKind::CollapsedWithWings(WING_LEFT | WING_RIGHT),
            &views.collapsed.root,
        );
        // Authorization prompts: an agent asking for access, a Spotify sign-in.
        views.peek.show(&prompts[0], &palette);
        show("07f-notched-auth-agent", LayoutKind::Peek, &views.peek.root);
        views.peek.show(&prompts[1], &palette);
        show("07g-notched-auth-spotify", LayoutKind::Peek, &views.peek.root);
        let mut asking = overview.clone();
        asking.notifications.splice(0..0, prompts.iter().cloned());
        asking.generation += 1;
        views.expanded.update(&asking);
        views.expanded.show_tab(Tab::Overview);
        show("07h-notched-auth-overview", LayoutKind::Expanded, &views.expanded.root);
        // The Agents page, with sample usage.
        let mut agents = overview.clone();
        agents.generation += 2;
        agents.usage = crate::selftest::sample_usage();
        views.expanded.update(&agents);
        views.expanded.show_tab(Tab::Agents);
        show("07i-notched-agents", LayoutKind::Expanded, &views.expanded.root);
        views.expanded.show_tab(Tab::Live);
        show("07q-notched-live", LayoutKind::Expanded, &views.expanded.root);
        // The Planner, with a sample day.
        let mut planner = agents.clone();
        planner.generation += 1;
        planner.calendar = crate::selftest::sample_calendar(planner.now_ms);
        views.expanded.update(&planner);
        views.expanded.show_tab(Tab::Planner);
        show("07r-notched-planner", LayoutKind::Expanded, &views.expanded.root);
        // The Shelf, with the sample tray (files and a shelved link).
        views.expanded.show_tab(Tab::Shelf);
        show("07s-notched-shelf", LayoutKind::Expanded, &views.expanded.root);
        // As from the desktop app alone: Claude Code's limits unknown, large totals.
        let mut desktop = agents.clone();
        desktop.generation += 1;
        desktop.usage.limits[0].gauges.clear();
        desktop.usage.limits[0].context_percent = Some(76);
        desktop.usage.limits[1].gauges.truncate(1);
        desktop.usage.limits[1].context_percent = None;
        desktop.usage.limits[1].running = false;
        desktop.usage.ranges[0].counts.input = 2_000;
        desktop.usage.ranges[0].counts.output = 2_350_000;
        desktop.usage.ranges[0].counts.cache_read = 443_800_000;
        views.expanded.update(&desktop);
        show("07l-notched-agents-desktop", LayoutKind::Expanded, &views.expanded.root);
        // A call just picked up, then on the hover card and the panel's pill.
        let on_call = crate::selftest::sample_active_call();
        views.call.update_active(&on_call, notchforge_core::time::now_ms());
        show("07m-notched-in-call", LayoutKind::Incoming(1), &views.call.root);
        views.expanded.show_tab(Tab::Overview);
        views.expanded.set_call(Some(&on_call), notchforge_core::time::now_ms());
        show("07o-notched-panel-in-call", LayoutKind::Expanded, &views.expanded.root);
        views.expanded.set_call(None, 0);
        // Calls ringing: one, then several stacked.
        let calls = crate::selftest::sample_calls();
        views.call.update(&calls[..1], false);
        show("07j-notched-call", LayoutKind::Incoming(1), &views.call.root);
        views.call.update(&calls, false);
        show("07k-notched-calls", LayoutKind::Incoming(3), &views.call.root);
        views.expanded.show_tab(Tab::Overview);
        views.expanded.media.set_active(false);
        // The floating capsule, as it renders on a notchless display: rounded
        // all round, hanging below the top edge, with the browser wings.
        let float_screen = notchforge_core::geometry::ScreenInfo {
            id: 7,
            frame: Rect::new(0.0, 0.0, 2560.0, 1440.0),
            visible_frame: Rect::new(0.0, 0.0, 2560.0, 1415.0),
            safe_area_top: 0.0,
            aux_top_left: None,
            aux_top_right: None,
            backing_scale: 1.0,
            is_builtin: false,
            is_main: false,
        };
        let float_layout = NotchLayout::new(float_screen, cfg.appearance.clone());
        let float_view = crate::views::collapsed::CollapsedView::new(self.mtm, &float_layout, &palette);
        float_view.update(
            &browser,
            &palette,
            crate::views::collapsed::WingOptions {
                media: true,
                animate: false,
                mask: WING_LEFT | WING_RIGHT,
                chevrons: 0,
            },
        );
        let fs = float_layout.shape(LayoutKind::CollapsedWithWings(WING_LEFT | WING_RIGHT));
        float_view.root.setHidden(false);
        float_view.root.setFrameOrigin(NSPoint::new(0.0, fs.height - float_view.root.frame().size.height));
        let float_island = NSView::initWithFrame(NSView::alloc(self.mtm), rect(0.0, 0.0, fs.width, fs.height));
        float_island.setWantsLayer(true);
        if let Some(l) = float_island.layer() {
            l.setMasksToBounds(true);
            l.setCornerRadius(fs.corner_radius);
            l.setBackgroundColor(Some(&objc2_app_kit::NSColor::blackColor().CGColor()));
        }
        float_island.addSubview(&float_view.root);
        let _material = crate::glass::Glass::preview(
            self.mtm,
            &float_island,
            fs.width,
            fs.height,
            fs.corner_radius,
            fs.height,
            0.0,
        );
        // SAFETY: a plain borderless window; closed and dropped below.
        let float_window = unsafe {
            let w = NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(self.mtm),
                rect(-20000.0, -20000.0, fs.width, fs.height),
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            );
            w.setReleasedWhenClosed(false);
            w.setContentView(Some(&float_island));
            w.orderFrontRegardless();
            w.displayIfNeeded();
            w
        };
        objc2_quartz_core::CATransaction::flush();
        render("07p-notched-floating", &float_island, fs.width, fs.height);
        float_window.close();
    }
}
