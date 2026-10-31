//! The notch controller: glues the pure [`NotchMachine`] and [`SpringRect`]
//! from `notchforge-core` to AppKit.
//!
//! Responsibilities:
//! * turn pointer/click/drag/key input into machine [`Event`]s,
//! * execute machine [`Effect`]s (layout changes, timers, interactivity, focus),
//! * drive the spring animation with a 120 Hz `NSTimer` that exists **only while
//!   something is moving** (zero CPU when idle),
//! * keep the panel window as small as possible when settled, so the island
//!   never blocks clicks on the menu bar outside its own shape,
//! * render snapshots into the views (lazily for the expanded panel).

use crate::panel::{NotchPanel, RootView};
use crate::screen;
use crate::ui::{Palette, rect};
use crate::views::{Views, system_increase_contrast, system_reduce_motion};
use block2::RcBlock;
use notchforge_core::config::{Config, ReducedMotion};
use notchforge_core::engine::{Command, EngineHandle, Snapshot};
use notchforge_core::geometry::{LayoutKind, NotchLayout, Point, choose_screen};
use notchforge_core::machine::{Effect, Event, NotchMachine, State, TimerKind, Timings};
use notchforge_core::model::Notification;
use notchforge_core::spring::SpringRect;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationDidChangeScreenParametersNotification, NSEvent, NSEventMask, NSEventModifierFlags,
    NSEventType, NSPasteboard, NSPasteboardNameDrag, NSPasteboardTypeFileURL, NSView, NSWorkspace,
    NSWorkspaceDidWakeNotification,
};
use objc2_foundation::{NSPoint, NSRunLoop, NSRunLoopCommonModes, NSTimer};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const FRAME_INTERVAL: f64 = 1.0 / 120.0;
const KEY_ESCAPE: u16 = 53;
const KEY_COMMA: u16 = 43;

/// Frame statistics of the most recent transition (exposed for the self-test).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AnimStats {
    pub frames: u32,
    pub duration_secs: f64,
    pub max_frame_secs: f64,
}

impl AnimStats {
    pub fn fps(&self) -> f64 {
        if self.duration_secs > 0.0 { self.frames as f64 / self.duration_secs } else { 0.0 }
    }
}

pub struct Controller {
    mtm: MainThreadMarker,
    pub panel: Retained<NotchPanel>,
    root: Retained<RootView>,
    island: Retained<NSView>,
    views: RefCell<Views>,
    palette: RefCell<Palette>,
    machine: RefCell<NotchMachine>,
    spring: RefCell<SpringRect>,
    layout: RefCell<NotchLayout>,
    kind: Cell<LayoutKind>,
    timers: RefCell<HashMap<TimerKind, Retained<NSTimer>>>,
    anim: RefCell<Option<Retained<NSTimer>>>,
    last_tick: Cell<Option<Instant>>,
    anim_stats: Cell<AnimStats>,
    snapshot: RefCell<Arc<Snapshot>>,
    config: RefCell<Arc<Config>>,
    engine: EngineHandle,
    pointer_inside: Cell<bool>,
    drag_count_at_down: Cell<isize>,
    /// AppKit confirmed the drag is over our window (draggingEntered:).
    native_drag: Cell<bool>,
    expanded_dirty: Cell<bool>,
    monitors: RefCell<Vec<Retained<AnyObject>>>,
    observers: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
}

fn timings(cfg: &Config) -> Timings {
    Timings {
        hover_intent: Duration::from_millis(cfg.general.hover_delay_ms),
        collapse_grace: Duration::from_millis(cfg.general.collapse_delay_ms),
        peek: Duration::from_millis(cfg.general.peek_duration_ms),
        drag_exit_grace: Duration::from_millis(600),
    }
}

fn current_layout(mtm: MainThreadMarker, cfg: &Config) -> Option<NotchLayout> {
    let screens = screen::screens(mtm);
    let mouse = NSEvent::mouseLocation();
    let idx = choose_screen(&screens, cfg.general.screen, Some(Point { x: mouse.x, y: mouse.y }))?;
    Some(NotchLayout::new(screens[idx].clone(), cfg.appearance.clone()))
}

impl Controller {
    pub fn new(mtm: MainThreadMarker, config: Arc<Config>, engine: EngineHandle) -> Result<Rc<Self>, String> {
        let layout = current_layout(mtm, &config).ok_or("no screens attached")?;
        let notch = layout.notch.rect;
        let frame = screen::to_nsrect(notch);
        let root = RootView::create(mtm, rect(0.0, 0.0, frame.size.width, frame.size.height));
        root.setAutoresizesSubviews(false);

        let island = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, notch.width, notch.height));
        island.setWantsLayer(true);
        if let Some(layer) = island.layer() {
            layer.setBackgroundColor(Some(&objc2_app_kit::NSColor::blackColor().CGColor()));
            layer.setMasksToBounds(true);
            // All corners are rounded, but the island extends one radius above
            // the window's top edge (see `apply_frame`), so the window clips the
            // top corners and the island merges squarely with the bezel.
        }
        root.addSubview(&island);

        let views = Views::new(mtm, &layout, &config, &engine);
        for v in views.all() {
            v.setHidden(true);
            island.addSubview(v);
        }
        let panel = NotchPanel::create(mtm, frame, &root);
        let shape = layout.shape(LayoutKind::Collapsed);
        let palette = Palette::new(config.appearance.high_contrast || system_increase_contrast());

        let this = Rc::new(Self {
            mtm,
            panel,
            root,
            island,
            views: RefCell::new(views),
            palette: RefCell::new(palette),
            machine: RefCell::new(NotchMachine::new(timings(&config))),
            spring: RefCell::new(SpringRect::new(shape.width, shape.height, shape.corner_radius)),
            layout: RefCell::new(layout),
            kind: Cell::new(LayoutKind::Collapsed),
            timers: RefCell::new(HashMap::new()),
            anim: RefCell::new(None),
            last_tick: Cell::new(None),
            anim_stats: Cell::new(AnimStats::default()),
            snapshot: RefCell::new(Arc::new(Snapshot::default())),
            config: RefCell::new(config),
            engine,
            pointer_inside: Cell::new(false),
            drag_count_at_down: Cell::new(0),
            native_drag: Cell::new(false),
            expanded_dirty: Cell::new(true),
            monitors: RefCell::new(Vec::new()),
            observers: RefCell::new(Vec::new()),
        });
        this.show_container(LayoutKind::Collapsed);
        this.apply_frame();
        this.settle_window();
        this.panel.orderFrontRegardless();
        Ok(this)
    }

    /// Install global event monitors and system observers.
    pub fn install(&self) {
        // Global monitors see events destined for *other* apps. They do not
        // need Accessibility permission for mouse events.
        let mask = NSEventMask::MouseMoved
            | NSEventMask::LeftMouseDragged
            | NSEventMask::LeftMouseDown
            | NSEventMask::LeftMouseUp
            | NSEventMask::RightMouseDown;
        let block = RcBlock::new(|event: NonNull<NSEvent>| {
            // SAFETY: AppKit passes a valid event for the duration of the call.
            let event = unsafe { event.as_ref() };
            crate::app::with_controller(|c| c.global_event(event));
        });
        if let Some(m) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &block) {
            self.monitors.borrow_mut().push(m);
        }

        let center = objc2_foundation::NSNotificationCenter::defaultCenter();
        let on_screens = RcBlock::new(|_n: NonNull<objc2_foundation::NSNotification>| {
            crate::app::with_controller(|c| c.relayout());
        });
        // SAFETY: valid notification name; the block is retained by the center.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSApplicationDidChangeScreenParametersNotification),
                None,
                None,
                &on_screens,
            )
        };
        self.observers.borrow_mut().push(token);

        let ws_center = NSWorkspace::sharedWorkspace().notificationCenter();
        let on_wake = RcBlock::new(|_n: NonNull<objc2_foundation::NSNotification>| {
            crate::app::with_controller(|c| {
                c.handle(Event::Reset);
                c.relayout();
                c.engine.send(Command::Refresh);
            });
        });
        // SAFETY: valid notification name; the block is retained by the center.
        let token = unsafe {
            ws_center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidWakeNotification),
                None,
                None,
                &on_wake,
            )
        };
        self.observers.borrow_mut().push(token);
    }

    // ── state machine plumbing ───────────────────────────────────────────

    pub fn handle(&self, event: Event) {
        let effects = self.machine.borrow_mut().handle(event);
        for e in effects {
            self.apply_effect(e);
        }
    }

    fn apply_effect(&self, e: Effect) {
        match e {
            Effect::SetLayout(kind) => self.set_layout(kind),
            Effect::Schedule(kind, after) => self.schedule(kind, after),
            Effect::Cancel(kind) => {
                if let Some(t) = self.timers.borrow_mut().remove(&kind) {
                    t.invalidate();
                }
            }
            Effect::SetInteractive(on) => self.panel.setIgnoresMouseEvents(!on),
            Effect::SetKey(on) => {
                if on {
                    self.panel.makeKeyWindow();
                    self.panel.makeFirstResponder(Some(&self.root));
                } else if self.panel.isKeyWindow() {
                    self.panel.resignKeyWindow();
                }
            }
            Effect::MarkSeen => {
                self.engine.send(Command::MarkAllRead);
                self.engine.send(Command::Refresh);
            }
        }
    }

    fn schedule(&self, kind: TimerKind, after: Duration) {
        let block = RcBlock::new(move |_t: NonNull<NSTimer>| {
            crate::app::with_controller(|c| {
                c.timers.borrow_mut().remove(&kind);
                c.handle(Event::TimerFired(kind));
            });
        });
        // SAFETY: documented NSTimer API; the block is copied by the timer.
        let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(after.as_secs_f64(), false, &block) };
        // Common modes so timers also fire during drag sessions and menu tracking.
        // SAFETY: valid timer and run loop mode constant.
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
        if let Some(old) = self.timers.borrow_mut().insert(kind, timer) {
            old.invalidate();
        }
    }

    // ── layout & animation ───────────────────────────────────────────────

    fn reduce_motion(&self) -> bool {
        match self.config.borrow().appearance.reduced_motion {
            ReducedMotion::Always => true,
            ReducedMotion::Never => false,
            ReducedMotion::System => system_reduce_motion(),
        }
    }

    fn show_container(&self, kind: LayoutKind) {
        let views = self.views.borrow();
        let active = views.container(kind);
        for v in views.all() {
            v.setHidden(!std::ptr::eq(&**v, &**active));
        }
    }

    fn set_layout(&self, kind: LayoutKind) {
        let previous = self.kind.replace(kind);
        let shape = self.layout.borrow().shape(kind);
        let speed = self.config.borrow().appearance.animation_speed;
        if kind == LayoutKind::Expanded {
            self.render_expanded(true);
        } else {
            self.views.borrow().expanded.set_drop_highlight(false);
        }
        self.views.borrow().expanded.set_visible(kind == LayoutKind::Expanded);
        if kind == LayoutKind::Compact {
            let snap = self.snapshot.borrow().clone();
            self.views.borrow().compact.update(&snap, &self.palette.borrow());
        }
        self.show_container(kind);
        {
            let mut s = self.spring.borrow_mut();
            s.animate_to(shape.width, shape.height, shape.corner_radius, speed);
            if previous != kind {
                s.content_alpha.value = 0.0;
                s.set_content_alpha(1.0, speed);
            }
            if self.reduce_motion() {
                s.snap();
            }
        }
        // Give the animation room: the window grows to its maximum while moving
        // and shrinks back around the island once settled.
        let max = self.layout.borrow().window_frame();
        self.panel.setFrame_display(screen::to_nsrect(max), false);
        self.root.setFrame(rect(0.0, 0.0, max.width, max.height));
        self.apply_frame();
        self.start_animation();
    }

    fn start_animation(&self) {
        if self.spring.borrow().is_settled() {
            self.settle_window();
            return;
        }
        if self.anim.borrow().is_some() {
            return;
        }
        self.last_tick.set(None);
        self.anim_stats.set(AnimStats::default());
        let block = RcBlock::new(|_t: NonNull<NSTimer>| {
            crate::app::with_controller(|c| c.tick());
        });
        // SAFETY: documented NSTimer API.
        let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(FRAME_INTERVAL, true, &block) };
        // SAFETY: valid timer and mode.
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
        *self.anim.borrow_mut() = Some(timer);
    }

    fn tick(&self) {
        let now = Instant::now();
        let dt = self.last_tick.replace(Some(now)).map_or(FRAME_INTERVAL, |t| (now - t).as_secs_f64());
        let mut st = self.anim_stats.get();
        st.frames += 1;
        st.duration_secs += dt;
        st.max_frame_secs = st.max_frame_secs.max(dt);
        self.anim_stats.set(st);
        self.spring.borrow_mut().step(dt);
        self.apply_frame();
        if self.spring.borrow().is_settled() {
            if let Some(t) = self.anim.borrow_mut().take() {
                t.invalidate();
            }
            self.settle_window();
        }
    }

    /// Position the island (and its active content) for the current spring values.
    fn apply_frame(&self) {
        let s = *self.spring.borrow();
        let (w, h) = (s.width.value.max(1.0), s.height.value.max(1.0));
        let g = self.layout.borrow().island_global(w, h);
        let win = self.panel.frame();
        let radius = s.radius.value.clamp(0.0, h / 2.0);
        // Extra height above the window's top edge, clipped away by the window.
        let overhang = radius.ceil();
        self.island.setFrame(rect(g.x - win.origin.x, g.y - win.origin.y, w, h + overhang));
        if let Some(layer) = self.island.layer() {
            layer.setCornerRadius(radius);
        }
        let views = self.views.borrow();
        let c = views.container(self.kind.get());
        let cs = c.frame().size;
        c.setFrameOrigin(NSPoint::new((w - cs.width) / 2.0, h - cs.height));
        c.setAlphaValue(s.content_alpha.value.clamp(0.0, 1.0));
    }

    /// Shrink the window to the island so nothing outside it intercepts clicks.
    fn settle_window(&self) {
        let s = *self.spring.borrow();
        let g = self.layout.borrow().island_global(s.width.value, s.height.value);
        self.panel.setFrame_display(screen::to_nsrect(g), true);
        self.root.setFrame(rect(0.0, 0.0, g.width, g.height));
        self.apply_frame();
    }

    /// Screen configuration changed: recompute geometry and rebuild views.
    pub fn relayout(&self) {
        let cfg = self.config.borrow().clone();
        let Some(layout) = current_layout(self.mtm, &cfg) else { return };
        if *self.layout.borrow() == layout {
            return;
        }
        tracing::info!(screen = layout.screen.id, notch = layout.notch.physical, "screen layout changed");
        *self.layout.borrow_mut() = layout;
        self.rebuild_views();
        let kind = self.kind.get();
        let shape = self.layout.borrow().shape(kind);
        {
            let mut s = self.spring.borrow_mut();
            s.animate_to(shape.width, shape.height, shape.corner_radius, 1.0);
            s.snap();
        }
        self.show_container(kind);
        self.settle_window();
    }

    fn rebuild_views(&self) {
        let cfg = self.config.borrow().clone();
        *self.palette.borrow_mut() = Palette::new(cfg.appearance.high_contrast || system_increase_contrast());
        for v in self.views.borrow().all() {
            v.removeFromSuperview();
        }
        let tab = self.views.borrow().expanded.tab();
        let views = Views::new(self.mtm, &self.layout.borrow(), &cfg, &self.engine);
        for v in views.all() {
            v.setHidden(true);
            self.island.addSubview(v);
        }
        views.expanded.show_tab(tab);
        views.expanded.set_visible(self.kind.get() == LayoutKind::Expanded);
        *self.views.borrow_mut() = views;
        self.expanded_dirty.set(true);
        let snap = self.snapshot.borrow().clone();
        self.render_small(&snap);
    }

    // ── input ────────────────────────────────────────────────────────────

    fn global_event(&self, event: &NSEvent) {
        let ty = event.r#type();
        if ty == NSEventType::MouseMoved {
            self.pointer_moved(NSEvent::mouseLocation());
        } else if ty == NSEventType::LeftMouseDragged {
            self.check_file_drag();
            self.pointer_moved(NSEvent::mouseLocation());
        } else if ty == NSEventType::LeftMouseDown || ty == NSEventType::RightMouseDown {
            // SAFETY: reading an AppKit constant.
            let pb = NSPasteboard::pasteboardWithName(unsafe { NSPasteboardNameDrag });
            self.drag_count_at_down.set(pb.changeCount());
            // A click in another app is always "outside" the island.
            if matches!(self.machine.borrow().state(), State::Expanded | State::Hover | State::Peek) {
                self.handle(Event::ClickOutside);
            }
        } else if ty == NSEventType::LeftMouseUp {
            // A drag that ended elsewhere (or was cancelled).
            if matches!(self.machine.borrow().state(), State::DropTarget { .. }) && !self.native_drag.get() {
                self.handle(Event::DragExited);
            }
        }
    }

    /// Detect a *new* file drag heading for the notch while the panel ignores
    /// mouse events (collapsed), and open the drop target.
    ///
    /// Until AppKit confirms the drag is over our window, the drop target is
    /// kept open by a self-re-arming grace timer: every drag event inside the
    /// zone re-arms it, so if the drag is released elsewhere (and no mouse-up
    /// reaches us during the system drag session) the panel still closes.
    fn check_file_drag(&self) {
        let m = NSEvent::mouseLocation();
        let in_zone = self.layout.borrow().drag_zone().contains_inclusive(Point { x: m.x, y: m.y });
        if matches!(self.machine.borrow().state(), State::DropTarget { .. }) {
            if in_zone && !self.native_drag.get() {
                self.handle(Event::DragEntered);
                self.handle(Event::DragExited);
            }
            return;
        }
        if !in_zone {
            return;
        }
        // SAFETY: reading AppKit constants.
        let pb = NSPasteboard::pasteboardWithName(unsafe { NSPasteboardNameDrag });
        if pb.changeCount() == self.drag_count_at_down.get() {
            return;
        }
        let has_files = pb.types().is_some_and(|types| types.iter().any(|t| &*t == unsafe { NSPasteboardTypeFileURL }));
        if has_files {
            self.handle(Event::DragEntered);
            self.handle(Event::DragExited);
            self.views.borrow().expanded.set_drop_highlight(true);
        }
    }

    pub fn pointer_moved(&self, p: NSPoint) {
        let p = Point { x: p.x, y: p.y };
        let state = self.machine.borrow().state();
        let inside = {
            let layout = self.layout.borrow();
            match state {
                State::Collapsed | State::Arming => layout.activation_zone().contains_inclusive(p),
                State::Peek => {
                    layout.activation_zone().contains_inclusive(p)
                        || layout.retention_zone(LayoutKind::Peek).contains_inclusive(p)
                }
                State::Hover => layout.retention_zone(LayoutKind::Compact).contains_inclusive(p),
                State::Expanded | State::DropTarget { .. } => {
                    layout.retention_zone(LayoutKind::Expanded).contains_inclusive(p)
                }
            }
        };
        if inside != self.pointer_inside.replace(inside) {
            self.handle(if inside { Event::PointerEnteredZone } else { Event::PointerLeftZone });
        }
    }

    pub fn root_mouse_down(&self, window_point: NSPoint) {
        let win = self.panel.frame();
        let global = Point { x: win.origin.x + window_point.x, y: win.origin.y + window_point.y };
        let s = *self.spring.borrow();
        let island = self.layout.borrow().island_global(s.width.value, s.height.value);
        if island.contains_inclusive(global) {
            self.handle(Event::Click);
        } else {
            self.handle(Event::ClickOutside);
        }
    }

    /// Called by the panel for every left mouse-down. Returns `true` when the
    /// click was handled here (pinning a hover/peek island).
    pub fn intercept_mouse_down(&self, window_point: NSPoint) -> bool {
        if matches!(self.machine.borrow().state(), State::Expanded | State::DropTarget { .. }) {
            return false;
        }
        // The peek's Open button opens the notification itself.
        if self.kind.get() == LayoutKind::Peek && self.views.borrow().peek.open_hit(window_point) {
            self.views.borrow().peek.open_current();
            self.handle(Event::ClickOutside);
            return true;
        }
        // Clicking the hover card's "now playing" opens straight on the music page.
        if self.kind.get() == LayoutKind::Compact && self.views.borrow().compact.media_hit(window_point) {
            self.views.borrow().expanded.show_tab(crate::views::expanded::Tab::Media);
        }
        self.root_mouse_down(window_point);
        true
    }

    // ── media page ───────────────────────────────────────────────────────

    /// The resolution pill was clicked: offer the display's modes.
    pub fn show_resolution_menu(&self, anchor: &objc2_app_kit::NSButton) {
        self.views.borrow().expanded.resolution_menu(anchor);
    }

    pub fn show_expanded_tab(&self, tab: crate::views::expanded::Tab) {
        self.views.borrow().expanded.show_tab(tab);
    }

    /// 1 Hz tick from the music page (progress bar and clocks).
    pub fn media_tick(&self) {
        self.views.borrow().expanded.media.tick();
    }

    /// Render the hover glance from an arbitrary snapshot (self-test).
    pub fn preview_compact(&self, snap: &Snapshot) {
        self.views.borrow().compact.update(snap, &self.palette.borrow());
    }

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
                l.setBackgroundColor(Some(&objc2_app_kit::NSColor::blackColor().CGColor()));
                l.setMasksToBounds(true);
                // On screen the top corners sit above the display edge; only
                // the lower two are visible.
                l.setCornerRadius(s.corner_radius);
                l.setMaskedCorners(
                    objc2_quartz_core::CACornerMask::LayerMinXMinYCorner
                        | objc2_quartz_core::CACornerMask::LayerMaxXMinYCorner,
                );
            }
            content.setHidden(false);
            island.addSubview(content);
            window.setContentView(Some(&island));
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
        views.expanded.media.update(music);
        views.expanded.show_tab(Tab::Media);
        show("07b-notched-music", LayoutKind::Expanded, &views.expanded.root);
        views.compact.update(music, &palette);
        show("07c-notched-hover", LayoutKind::Compact, &views.compact.root);
        views.peek.show(peek, &palette);
        show("07d-notched-peek", LayoutKind::Peek, &views.peek.root);
        views.expanded.media.set_active(false);
    }

    pub fn media_page_visible(&self) -> bool {
        let views = self.views.borrow();
        views.expanded.tab() == crate::views::expanded::Tab::Media && !views.expanded.media.root.isHidden()
    }

    /// Render the music page from an arbitrary snapshot (self-test). Returns
    /// the displayed title and the number of playlist rows.
    pub fn preview_media(&self, snap: &Snapshot) -> (String, usize) {
        let views = self.views.borrow();
        views.expanded.media.invalidate();
        views.expanded.media.update(snap);
        views.expanded.media.debug_summary()
    }

    /// Re-render the music page from the current snapshot (after a local
    /// choice such as switching the playlist source).
    pub fn media_rerender(&self) {
        let snap = self.snapshot.borrow().clone();
        let views = self.views.borrow();
        views.expanded.media.invalidate();
        views.expanded.media.update(&snap);
    }

    pub fn key_down(&self, key: u16, flags: NSEventModifierFlags) -> bool {
        match key {
            KEY_ESCAPE => {
                self.handle(Event::Escape);
                true
            }
            KEY_COMMA if flags.contains(NSEventModifierFlags::Command) => {
                crate::app::show_settings();
                true
            }
            _ => false,
        }
    }

    pub fn drag_entered(&self) {
        self.native_drag.set(true);
        self.handle(Event::DragEntered);
        self.views.borrow().expanded.set_drop_highlight(true);
    }

    pub fn drag_exited(&self) {
        self.native_drag.set(false);
        self.handle(Event::DragExited);
        self.views.borrow().expanded.set_drop_highlight(false);
    }

    pub fn files_dropped(&self, paths: Vec<PathBuf>) {
        tracing::info!(count = paths.len(), "files dropped on notch");
        self.native_drag.set(false);
        self.engine.send(Command::TrayAdd(paths));
        self.views.borrow().expanded.set_drop_highlight(false);
        self.handle(Event::Dropped);
    }

    // ── public actions (menu bar, buttons) ───────────────────────────────

    pub fn toggle(&self) {
        self.handle(Event::Toggle);
    }

    pub fn collapse(&self) {
        self.handle(Event::Escape);
    }

    pub fn toggle_pause(&self) {
        let paused = self.snapshot.borrow().paused;
        self.engine.send(Command::SetPaused(!paused));
    }

    pub fn is_paused(&self) -> bool {
        self.snapshot.borrow().paused
    }

    pub fn tray_paths(&self) -> Vec<PathBuf> {
        self.views.borrow().expanded.tray_paths()
    }

    pub fn toggle_focus(&self) {
        let cfg = self.config.borrow().clone();
        let active = self.snapshot.borrow().focus.active;
        let on = &cfg.controls.focus_on_shortcut;
        let off = &cfg.controls.focus_off_shortcut;
        let missing: Vec<&String> =
            [on, off].into_iter().filter(|n| !crate::controls::focus::shortcut_exists(n)).collect();
        if !missing.is_empty() {
            crate::app::alert(
                "Set up Focus shortcuts",
                &format!(
                    "macOS doesn't let apps toggle Focus directly. Create these shortcuts in the Shortcuts app with the \
                     “Set Focus” action, then try again:\n\n• {}\n• {}\n\nThe names can be changed in Settings.",
                    on, off
                ),
            );
            return;
        }
        if let Err(e) = crate::controls::focus::toggle(active, on, off) {
            crate::app::alert("Could not toggle Focus", &e);
        }
    }

    /// Resolution popup changed: apply the mode, then ask to keep or revert.
    pub fn choose_display_mode(&self, idx: isize) {
        let Some((display, mode)) = self.views.borrow().expanded.display_mode_at(idx) else { return };
        match crate::controls::display_modes::apply(display, mode) {
            Ok(Some(previous)) if previous != mode => {
                let secs = self.config.borrow().controls.resolution_confirm_secs;
                if !crate::app::confirm_with_timeout(
                    "Keep this display resolution?",
                    "The previous resolution will be restored automatically.",
                    "Keep",
                    "Revert",
                    secs,
                ) {
                    if let Err(e) = crate::controls::display_modes::apply(display, previous) {
                        tracing::warn!(error = %e, "could not revert display mode");
                    }
                }
            }
            Ok(_) => {}
            Err(e) => crate::app::alert("Could not change resolution", &e),
        }
        let id = self.layout.borrow().screen.id;
        self.views.borrow().expanded.refresh_controls(id);
    }

    // ── snapshots & config ───────────────────────────────────────────────

    pub fn apply_snapshot(&self, snap: Arc<Snapshot>) {
        *self.snapshot.borrow_mut() = snap.clone();
        self.render_small(&snap);
        let wings = {
            let cfg = self.config.borrow();
            cfg.appearance.show_wings && snap.wants_wings_with(cfg.media.show_in_wings)
        };
        self.handle(Event::WingsChanged(wings));
        self.expanded_dirty.set(true);
        if self.kind.get() == LayoutKind::Expanded {
            self.render_expanded(false);
        }
    }

    fn render_small(&self, snap: &Snapshot) {
        let views = self.views.borrow();
        let palette = self.palette.borrow();
        let opts = crate::views::collapsed::WingOptions {
            media: self.config.borrow().media.show_in_wings,
            animate: !self.reduce_motion(),
        };
        views.collapsed.update(snap, &palette, opts);
        if self.kind.get() == LayoutKind::Compact {
            views.compact.update(snap, &palette);
        }
    }

    fn render_expanded(&self, opening: bool) {
        if self.expanded_dirty.replace(false) {
            let snap = self.snapshot.borrow().clone();
            self.views.borrow().expanded.update(&snap);
        }
        if opening {
            let id = self.layout.borrow().screen.id;
            self.views.borrow().expanded.refresh_controls(id);
        }
    }

    pub fn peek(&self, n: Notification) {
        if self.machine.borrow().is_open() {
            return;
        }
        self.views.borrow().peek.show(&n, &self.palette.borrow());
        self.handle(Event::NotificationArrived);
    }

    pub fn apply_config(&self, cfg: Arc<Config>) {
        let old = self.config.replace(cfg.clone());
        self.machine.borrow_mut().set_timings(timings(&cfg));
        self.views.borrow().expanded.set_config(cfg.clone());
        if old.appearance != cfg.appearance || old.general.screen != cfg.general.screen {
            // Force a geometry + view rebuild.
            if let Some(layout) = current_layout(self.mtm, &cfg) {
                *self.layout.borrow_mut() = layout;
            }
            self.rebuild_views();
            self.handle(Event::Reset);
            let shape = self.layout.borrow().shape(self.kind.get());
            {
                let mut s = self.spring.borrow_mut();
                s.animate_to(shape.width, shape.height, shape.corner_radius, 1.0);
                s.snap();
            }
            self.show_container(self.kind.get());
            self.settle_window();
        }
        let wings = cfg.appearance.show_wings && self.snapshot.borrow().wants_wings_with(cfg.media.show_in_wings);
        self.handle(Event::WingsChanged(wings));
    }

    // ── introspection (self-test & diagnostics) ──────────────────────────

    pub fn machine_state(&self) -> State {
        self.machine.borrow().state()
    }

    pub fn layout_kind(&self) -> LayoutKind {
        self.kind.get()
    }

    pub fn layout(&self) -> NotchLayout {
        self.layout.borrow().clone()
    }

    pub fn is_animating(&self) -> bool {
        self.anim.borrow().is_some()
    }

    pub fn last_animation(&self) -> AnimStats {
        self.anim_stats.get()
    }

    /// Island rect in global coordinates as currently laid out.
    /// Visible island rect in global coordinates (the part inside the window;
    /// the overhang above the top edge is excluded).
    pub fn island_global(&self) -> notchforge_core::geometry::Rect {
        let win = self.panel.frame();
        let f = self.island.frame();
        let visible_h = f.size.height.min(win.size.height - f.origin.y).max(0.0);
        notchforge_core::geometry::Rect::new(
            win.origin.x + f.origin.x,
            win.origin.y + f.origin.y,
            f.size.width,
            visible_h,
        )
    }

    pub fn panel_global(&self) -> notchforge_core::geometry::Rect {
        screen::to_rect(self.panel.frame())
    }

    pub fn island_layer(&self) -> Option<Retained<objc2_quartz_core::CALayer>> {
        self.island.layer()
    }

    pub fn ignores_mouse(&self) -> bool {
        self.panel.ignoresMouseEvents()
    }

    pub fn current_snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.borrow().clone()
    }

    pub fn screen_id(&self) -> u32 {
        self.layout.borrow().screen.id
    }

    pub fn shutdown(&self) {
        for m in self.monitors.borrow_mut().drain(..) {
            // SAFETY: monitor tokens returned by addGlobalMonitor….
            unsafe { NSEvent::removeMonitor(&m) };
        }
        for t in self.timers.borrow_mut().drain() {
            t.1.invalidate();
        }
        if let Some(t) = self.anim.borrow_mut().take() {
            t.invalidate();
        }
        let center = objc2_foundation::NSNotificationCenter::defaultCenter();
        for o in self.observers.borrow_mut().drain(..) {
            // SAFETY: tokens from addObserverForName….
            unsafe { center.removeObserver((*o).as_ref()) };
        }
        self.panel.orderOut(None);
        let _ = NSApplication::sharedApplication(self.mtm);
    }
}
