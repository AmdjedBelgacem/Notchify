//! Display-only islands on the screens the primary one isn't on.
//!
//! Each mirror is a lightweight panel showing the collapsed state — the notch
//! or the floating capsule, with the same live wings and call indicators —
//! driven straight from the shared snapshot. No input: hover, panels and calls
//! stay on the primary screen, so there is exactly one source of truth and one
//! interactive window. A mirror hides itself while its own screen is covered
//! by a fullscreen app.

use crate::screen;
use crate::ui::{Palette, rect};
use notchforge_core::config::Config;
use notchforge_core::engine::Snapshot;
use notchforge_core::geometry::{LayoutKind, NotchLayout, ScreenInfo, WING_LEFT};
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSPanel, NSView, NSWindowCollectionBehavior, NSWindowStyleMask};
use std::sync::Arc;

struct MirrorIsland {
    screen_id: u32,
    panel: Retained<NSPanel>,
    island: Retained<NSView>,
    ears: [Retained<objc2_quartz_core::CAShapeLayer>; 2],
    collapsed: crate::views::collapsed::CollapsedView,
    layout: NotchLayout,
}

/// One mirror per screen the primary island isn't on.
#[derive(Default)]
pub struct Mirrors {
    islands: Vec<MirrorIsland>,
}

impl Mirrors {
    /// How many mirrors currently exist.
    pub fn count(&self) -> usize {
        self.islands.len()
    }

    /// Add or drop mirrors so every screen but `primary_id` has one. Existing
    /// mirrors whose screen hasn't changed are kept.
    pub fn sync(
        &mut self,
        mtm: MainThreadMarker,
        screens: &[ScreenInfo],
        primary_id: u32,
        cfg: &Arc<Config>,
        palette: &Palette,
    ) {
        let listed = &cfg.general.mirror_screens;
        if !cfg.general.island_on_all_screens && listed.is_empty() {
            self.islands.clear();
            return;
        }
        let wanted: Vec<&ScreenInfo> = screens
            .iter()
            .filter(|s| {
                s.id != primary_id
                    && (cfg.general.island_on_all_screens || listed.contains(&s.id))
            })
            .collect();
        // Drop mirrors whose screen is gone (or whose geometry changed).
        self.islands.retain(|m| {
            wanted.iter().any(|s| {
                let layout = NotchLayout::new((*s).clone(), cfg.appearance.clone());
                m.screen_id == s.id && m.layout.notch == layout.notch
            })
        });
        for s in wanted {
            let layout = NotchLayout::new(s.clone(), cfg.appearance.clone());
            if self.islands.iter().any(|m| m.screen_id == s.id && m.layout.notch == layout.notch) {
                continue;
            }
            match MirrorIsland::new(mtm, layout, cfg, palette) {
                Some(m) => self.islands.push(m),
                None => tracing::warn!(screen = s.id, "could not create a mirror island"),
            }
        }
    }

    /// Render the collapsed state on every mirror and hide the ones whose
    /// screen is fullscreen-covered. Each mirror drops its own wings per its
    /// screen's occupancy (menu bar items, the camera pill) — it does not
    /// inherit the primary's mask, since screens differ in what sits beside
    /// them.
    pub fn update(
        &mut self,
        mtm: MainThreadMarker,
        snap: &Snapshot,
        palette: &Palette,
        wings: crate::controller::WingDecision<'_>,
        primary_h: f64,
    ) {
        let crate::controller::WingDecision { content_mask, opts, base_occupancy } = wings;
        let windows = crate::windows::on_screen(mtm);
        for m in &mut self.islands {
            // The same smart placement as the primary's floating pill.
            if m.layout.notch.floating {
                let frame = m.layout.screen.frame;
                let occ: Vec<(f64, f64)> = if crate::accessibility::is_trusted() {
                    let band_top = frame.max_y() - m.layout.screen.menu_bar_height();
                    let mut occ = crate::accessibility::app_menu_rects(primary_h);
                    occ.extend(crate::accessibility::status_item_rects(primary_h));
                    occ.extend(crate::windows::status_item_rects(&windows));
                    occ.into_iter()
                        .filter(|r| r.max_y() > band_top && r.y < frame.max_y())
                        .map(|r| (r.x - frame.x, r.max_x() - frame.x))
                        .collect()
                } else {
                    Vec::new()
                };
                let x = notchforge_core::geometry::pill_x(frame.width, m.layout.notch.rect.width, &occ);
                m.layout.x_offset = (frame.x + x + m.layout.notch.rect.width / 2.0) - m.layout.notch.rect.mid_x();
            }
            // Per-screen wing mask: base occupancy plus this screen's camera
            // pill strip, applied to the layout's own notch.
            let mut occupancy = base_occupancy.to_vec();
            if crate::private_api::cmio::camera_in_use()
                || notchforge_providers::sys::coreaudio::microphone_in_use()
            {
                let notch = m.layout.notch.rect;
                occupancy.push(notchforge_core::geometry::Rect::new(
                    notch.max_x(),
                    notch.y,
                    40.0,
                    notch.height,
                ));
            }
            let mask = crate::controller::wing_mask_for(&m.layout, content_mask, &occupancy);
            let shape = m.layout.shape(LayoutKind::CollapsedWithWings(mask));
            let (w, h) = (shape.width.max(1.0), shape.height.max(1.0));
            let radius = if m.layout.notch.floating { h / 2.0 } else { shape.corner_radius };
            // Shrink the window around the island, like the primary settles:
            // the flares need room beside a physical notch, the capsule just a
            // pixel or two so its rounded corners are not clipped.
            let g = m.layout.island_global(w, h);
            let pad = if m.layout.notch.floating { 2.0 } else { notchforge_core::geometry::NOTCH_EAR_MAX };
            let win_rect = notchforge_core::geometry::Rect::new(g.x - pad, g.y, w + 2.0 * pad, h);
            m.panel.setFrame_display(screen::to_nsrect(win_rect), true);
            if let Some(cv) = m.panel.contentView() {
                cv.setFrame(rect(0.0, 0.0, win_rect.width, win_rect.height));
            }
            let win = m.panel.frame();
            // A physical notch merges with the bezel: the island extends one
            // radius above the window's top edge, which clips the rounded top
            // corners away. The capsule stays whole.
            let overhang = if m.layout.notch.floating { 0.0 } else { radius.ceil() };
            m.island.setFrame(rect(g.x - win.origin.x, g.y - win.origin.y, w, h + overhang));
            if let Some(layer) = m.island.layer() {
                layer.setCornerRadius(radius);
            }
            if m.layout.notch.floating {
                for ear in &m.ears {
                    ear.setHidden(true);
                }
            } else {
                let e = notchforge_core::geometry::notch_ear_radius(radius);
                for (ear, left) in m.ears.iter().zip([true, false]) {
                    ear.setFrame(rect(
                        if left { g.x - win.origin.x - e } else { g.x - win.origin.x + w },
                        win.size.height - e,
                        e,
                        e,
                    ));
                    ear.setHidden(false);
                }
            }
            let collapsed_container = m.collapsed.root.clone();
            let cs = collapsed_container.frame().size;
            let ox = if mask & WING_LEFT == 0 && cs.width > w { -(cs.width - w) } else { (w - cs.width) / 2.0 };
            collapsed_container.setFrameOrigin(objc2_foundation::NSPoint::new(ox, h - cs.height));
            let opts = crate::views::collapsed::WingOptions { media: opts.media, animate: false, mask };
            m.collapsed.update(snap, palette, opts);
            // Out of the way while this screen is fullscreen-covered.
            let covered = crate::windows::fullscreen_covers(&windows, &m.layout.screen);
            if covered == m.panel.isVisible() {
                if covered {
                    m.panel.orderOut(None);
                } else {
                    m.panel.orderFrontRegardless();
                }
            }
        }
    }

    pub fn shutdown(&mut self) {
        self.islands.clear();
    }
}

impl MirrorIsland {
    fn new(mtm: MainThreadMarker, layout: NotchLayout, _cfg: &Arc<Config>, palette: &Palette) -> Option<Self> {
        let notch = layout.notch.rect;
        // A plain (y-up) view, like the primary's root: the island is
        // bottom-anchored and its overhang sticks out the window's top, where
        // the window clips the rounded top corners away (square bezel merge).
        // A flipped root here would render the whole notch upside down.
        let root = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, notch.width, notch.height));
        root.setWantsLayer(true);
        let island = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, notch.width, notch.height));
        island.setWantsLayer(true);
        if let Some(layer) = island.layer() {
            layer.setMasksToBounds(true);
            layer.setBackgroundColor(Some(&objc2_app_kit::NSColor::blackColor().CGColor()));
        }
        root.addSubview(&island);
        let ears = [true, false].map(|left| {
            let l = objc2_quartz_core::CAShapeLayer::layer();
            l.setFillColor(Some(&objc2_app_kit::NSColor::blackColor().CGColor()));
            if let Some(root_layer) = root.layer() {
                root_layer.addSublayer(&l);
            }
            crate::ui::set_notch_flare(&l, notchforge_core::geometry::NOTCH_EAR_MAX, left);
            l
        });
        let collapsed = crate::views::collapsed::CollapsedView::new(mtm, &layout, palette);
        collapsed.root.setHidden(false);
        island.addSubview(&collapsed.root);
        if let Some(l) = root.layer() {
            crate::glass::smooth_edges(&l);
        }

        let win_frame = layout.window_frame();
        // SAFETY: a plain borderless panel; display-only and never key.
        let panel = unsafe {
            let p = NSPanel::initWithContentRect_styleMask_backing_defer(
                NSPanel::alloc(mtm),
                screen::to_nsrect(win_frame),
                NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                objc2_app_kit::NSBackingStoreType::Buffered,
                false,
            );
            p.setLevel(objc2_app_kit::NSMainMenuWindowLevel + 3);
            p.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::IgnoresCycle,
            );
            p.setOpaque(false);
            p.setBackgroundColor(Some(&objc2_app_kit::NSColor::clearColor()));
            p.setHasShadow(false);
            p.setIgnoresMouseEvents(true);
            p.setHidesOnDeactivate(false);
            p.setReleasedWhenClosed(false);
            p.setContentView(Some(&root));
            p
        };
        panel.orderFrontRegardless();
        Some(Self { screen_id: layout.screen.id, panel, island, ears, collapsed, layout })
    }
}

impl Drop for MirrorIsland {
    fn drop(&mut self) {
        self.panel.orderOut(None);
    }
}
