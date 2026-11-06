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
use notchforge_core::geometry::{
    LayoutKind, NotchLayout, Point, Rect, WING_LEFT, WING_RIGHT, WING_WIDTH, choose_screen,
};
use notchforge_core::machine::{Effect, Event, NotchMachine, State, TimerKind, Timings};
use notchforge_core::model::{MediaAction, MediaPlayer, Notification};
use notchforge_core::spring::SpringRect;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationDidChangeScreenParametersNotification, NSEvent, NSEventMask, NSEventModifierFlags,
    NSEventType, NSPasteboard, NSPasteboardNameDrag, NSPasteboardTypeFileURL, NSScreen, NSView, NSWorkspace,
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
use notchforge_core::time::now_ms;

const KEY_ESCAPE: u16 = 53;
/// How long the big view stays after picking up, before settling beside the notch.
const JUST_ANSWERED_MS: i64 = 5_000;
/// Pressing hang up again this soon, with no button found, means it's over.
const HANG_UP_AGAIN_MS: i64 = 20_000;
const KEY_COMMA: u16 = 43;
/// How long a paused track keeps the media wings (dimmed, static) so one
/// click on them resumes it, before the wings fold back to the indicators.
const PAUSED_WINGS_MS: i64 = 5 * 60 * 1000;

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

/// A computed wing decision: which sides have content, the render options,
/// and the menu bar occupancy to dodge against.
pub struct WingDecision<'a> {
    pub content_mask: u8,
    pub opts: crate::views::collapsed::WingOptions,
    pub base_occupancy: &'a [Rect],
}

/// Drop a content mask's wings whose span is occupied on that layout's
/// screen (menu bar items, the camera/mic pill, …). Shared by the primary
/// island and the mirrors.
pub fn wing_mask_for(layout: &NotchLayout, content: u8, occupancy: &[Rect]) -> u8 {
        let notch = layout.notch.rect;
        let mut mask = content;
        let left = Rect::new(notch.x - WING_WIDTH, notch.y, WING_WIDTH, notch.height);
        let right = Rect::new(notch.max_x(), notch.y, WING_WIDTH, notch.height);
        if occupancy.iter().any(|r| r.intersects(&left)) {
            mask &= !WING_LEFT;
        }
        if occupancy.iter().any(|r| r.intersects(&right)) {
            mask &= !WING_RIGHT;
        }
        mask
}

pub struct Controller {
    mtm: MainThreadMarker,
    pub panel: Retained<NotchPanel>,
    root: Retained<RootView>,
    island: Retained<NSView>,
    /// Concave flares either side of the island's top edge (left, right).
    ears: [Retained<objc2_quartz_core::CAShapeLayer>; 2],
    ear_radius: Cell<f64>,
    /// Black at the notch, Liquid Glass at the ends.
    glass: crate::glass::Glass,
    /// Height the current or last opening travels above the notch.
    glass_span: Cell<f64>,
    /// Calls are ringing and showing, on top of whatever the machine wants.
    calls_showing: Cell<bool>,
    /// The user hid the ringing calls; a new call or a hover brings them back.
    calls_hidden: Cell<bool>,
    /// When hang up last found no button to press.
    hang_up_tried_ms: Cell<i64>,
    known_calls: RefCell<Vec<notchforge_core::calls::CallId>>,
    call_timer: RefCell<Option<Retained<NSTimer>>>,
    /// What the machine last asked for, restored when calls stop.
    wanted_kind: Cell<LayoutKind>,
    wanted_interactive: Cell<bool>,
    /// A fullscreen app covers the island's screen (see `windows.rs`).
    fs_active: Cell<bool>,
    /// The wing mask last sent to the machine.
    wing_mask: Cell<u8>,
    /// Sides whose content wants a wing but is dodged beside menu bar items:
    /// they show the wing chevron (`WING_LEFT` / `WING_RIGHT` bits).
    wing_chevrons: Cell<u8>,
    /// A chevron click pops a dodged wing out over the items for a moment:
    /// the forced sides and when the pop-out started (0 = none).
    wing_override: Cell<(u8, i64)>,
    /// Re-checks menu bar occupancy while the wings are visible.
    wing_timer: RefCell<Option<Retained<NSTimer>>>,
    /// Set while a pass-through click's synthetic down/up is in flight, so the
    /// global monitor ignores our own events.
    repost_click: Cell<bool>,
    /// The floating pill's animated sideways offset from its centred spot.
    pill_x: RefCell<notchforge_core::spring::Spring>,
    /// Logged once: wings suppressed because Accessibility is missing.
    ax_note_logged: Cell<bool>,
    /// The message being replied to from the notch (reply bar open).
    reply_to: RefCell<Option<Notification>>,
    /// Last reported mirror count, so the sync log is one-shot per change.
    mirror_count: Cell<usize>,
    /// Display-only islands on the screens the primary isn't on.
    mirrors: RefCell<crate::mirrors::Mirrors>,
    /// The last time something was actually audible; a recently paused track
    /// keeps its wings for `PAUSED_WINGS_MS`.
    last_playing_ms: Cell<i64>,
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
    /// A screen pinned by clicking its mirror (mirror click-to-focus); wins
    /// over the configured screen choice until it disappears.
    focus_override: Cell<Option<u32>>,
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
    current_layout_pinned(mtm, cfg, None)
}

/// Like [`current_layout`], with an optional pinned screen id (mirror click).
fn current_layout_pinned(mtm: MainThreadMarker, cfg: &Config, pinned: Option<u32>) -> Option<NotchLayout> {
    let screens = screen::screens(mtm);
    let mouse = NSEvent::mouseLocation();
    let idx = choose_screen(&screens, cfg.general.screen, Some(Point { x: mouse.x, y: mouse.y }), pinned)?;
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
            layer.setMasksToBounds(true);
            // All corners are rounded, but the island extends one radius above
            // the window's top edge (see `apply_frame`), so the window clips the
            // top corners and the island merges squarely with the bezel.
        }
        root.addSubview(&island);
        // Flares: black, outside the island, so the notch widens into the
        // menu bar the way the hardware notch does.
        root.setWantsLayer(true);
        let ears = [true, false].map(|left| {
            let l = objc2_quartz_core::CAShapeLayer::layer();
            l.setFillColor(Some(&objc2_app_kit::NSColor::blackColor().CGColor()));
            if let Some(root_layer) = root.layer() {
                root_layer.addSublayer(&l);
            }
            crate::ui::set_notch_flare(&l, notchforge_core::geometry::NOTCH_EAR_MAX, left);
            l
        });

        let glass = crate::glass::Glass::install(mtm, &island, notch.height);
        if let Some(l) = root.layer() {
            crate::glass::smooth_edges(&l);
        }
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
            ears,
            ear_radius: Cell::new(0.0),
            glass,
            glass_span: Cell::new(0.0),
            calls_showing: Cell::new(false),
            calls_hidden: Cell::new(false),
            hang_up_tried_ms: Cell::new(0),
            known_calls: RefCell::new(Vec::new()),
            call_timer: RefCell::new(None),
            wanted_kind: Cell::new(LayoutKind::Collapsed),
            wanted_interactive: Cell::new(false),
            fs_active: Cell::new(false),
            wing_mask: Cell::new(0),
            wing_chevrons: Cell::new(0),
            wing_override: Cell::new((0, 0)),
            wing_timer: RefCell::new(None),
            repost_click: Cell::new(false),
            ax_note_logged: Cell::new(false),
            reply_to: RefCell::new(None),
            mirror_count: Cell::new(usize::MAX),
            mirrors: RefCell::new(crate::mirrors::Mirrors::default()),
            pill_x: RefCell::new(notchforge_core::spring::Spring::new(
                0.0,
                notchforge_core::spring::SpringParams::with_response(0.45, 0.9),
            )),
            last_playing_ms: Cell::new(0),
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
            focus_override: Cell::new(None),
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
            crate::app::with_controller(|c| {
                c.relayout();
                c.evaluate_fullscreen();
                c.refresh_wing_mask();
            });
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
                c.evaluate_fullscreen();
                c.refresh_wing_mask();
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

        // Space switches and app activations move fullscreen apps around and
        // change both the menu bar's contents and who owns it.
        let on_space_or_app = RcBlock::new(|_n: NonNull<objc2_foundation::NSNotification>| {
            crate::app::with_controller(|c| {
                c.evaluate_fullscreen();
                c.refresh_wing_mask();
            });
        });
        for name in ["NSWorkspaceActiveSpaceDidChangeNotification", "NSWorkspaceDidActivateApplicationNotification"] {
            let ns_name = objc2_foundation::NSString::from_str(name);
            // SAFETY: valid notification name; the block is retained by the center.
            let token = unsafe {
                ws_center.addObserverForName_object_queue_usingBlock(Some(&ns_name), None, None, &on_space_or_app)
            };
            self.observers.borrow_mut().push(token);
        }
    }

    // ── state machine plumbing ───────────────────────────────────────────

    pub fn handle(&self, event: Event) {
        let effects = self.machine.borrow_mut().handle(event);
        for e in effects {
            self.apply_effect(e);
        }
        self.update_visibility();
    }
    fn apply_effect(&self, e: Effect) {
        match e {
            Effect::SetLayout(kind) => {
                self.wanted_kind.set(kind);
                if !self.calls_showing.get() {
                    self.set_layout(kind);
                }
            }
            Effect::Schedule(kind, after) => self.schedule(kind, after),
            Effect::Cancel(kind) => {
                if let Some(t) = self.timers.borrow_mut().remove(&kind) {
                    t.invalidate();
                }
            }
            Effect::SetInteractive(on) => {
                self.wanted_interactive.set(on);
                self.update_interactive();
            }
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

    // ── fullscreen & visibility ──────────────────────────────────────────────

    /// A fullscreen app is frontmost and the user asked the island to hide.
    fn fullscreen_hides(&self) -> bool {
        self.fs_active.get() && self.config.borrow().general.hide_in_fullscreen
    }

    /// The panel is on screen unless a fullscreen app should push it out —
    /// and even then a ringing call, an open panel (toggled or dropped onto)
    /// or a drag in progress wins.
    fn update_visibility(&self) {
        let machine_open = self.machine.borrow().is_open();
        let visible = !self.fullscreen_hides() || self.calls_showing.get() || machine_open;
        if visible && !self.panel.isVisible() {
            self.panel.orderFrontRegardless();
        } else if !visible && self.panel.isVisible() {
            self.panel.orderOut(None);
        }
        self.update_interactive();
    }

    // ── quick media toggle ───────────────────────────────────────────────────

    /// While collapsed, a wing showing the playing audio (cover art, browser
    /// icon or equaliser) takes clicks so it can toggle playback. Everything
    /// else about the collapsed island stays click-through.
    pub fn media_wing_clickable(&self) -> bool {
        if !self.panel.isVisible() || self.calls_showing.get() {
            return false;
        }
        matches!(self.machine.borrow().state(), State::Collapsed | State::Arming)
            && matches!(self.kind.get(), LayoutKind::Collapsed | LayoutKind::CollapsedWithWings(_))
            && self.views.borrow().collapsed.media_showing()
    }

    /// The panel ignores mouse events unless the machine opened it, calls are
    /// showing, or the collapsed media wings are clickable.
    fn update_interactive(&self) {
        let on = self.wanted_interactive.get() || self.calls_showing.get() || self.media_wing_clickable();
        self.panel.setIgnoresMouseEvents(!on);
    }

    /// The playing audio was clicked in a collapsed wing: toggle it. What the
    /// wings show decides the target — a playing player is the sound they
    /// hear; a paused one (recently playing, dimmed) resumes; otherwise the
    /// audible app gets the system media keys.
    fn media_wing_clicked(&self) {
        let media_on = self.config.borrow().media.show_in_wings;
        let snap = self.snapshot.borrow().clone();
        let current = snap.media.primary().filter(|_| media_on);
        let audible = snap.audio.is_playing();
        match current {
            Some(n) if n.is_playing() => {
                tracing::info!(player = ?n.player, "quick toggle from the wing");
                crate::media::control(n.player, MediaAction::PlayPause);
            }
            Some(n) if !audible => {
                tracing::info!(player = ?n.player, "quick resume from the wing");
                crate::media::control(n.player, MediaAction::PlayPause);
            }
            _ if audible => {
                tracing::info!("quick toggle from the wing (media keys)");
                crate::media::control(MediaPlayer::Browser, MediaAction::PlayPause);
            }
            _ => {}
        }
    }

    /// Watchers: re-read whether a fullscreen app covers the island's screen.
    pub fn evaluate_fullscreen(&self) {
        let active = {
            let windows = crate::windows::on_screen(self.mtm);
            crate::windows::fullscreen_covers(&windows, &self.layout.borrow().screen)
        };
        self.set_fullscreen(active);
    }

    /// Enter or leave the fullscreen-hidden mode.
    pub fn set_fullscreen(&self, active: bool) {
        if self.fs_active.replace(active) == active {
            return;
        }
        tracing::info!(active, screen = self.layout.borrow().screen.id, "fullscreen app on the island's screen");
        self.apply_fullscreen_state();
    }

    /// Apply the hide rule to the current fullscreen state (also after a
    /// config change): collapse without the closing animation — the window is
    /// about to disappear — and let `update_visibility` order the panel out.
    fn apply_fullscreen_state(&self) {
        if self.fs_active.get() && self.config.borrow().general.hide_in_fullscreen {
            self.handle(Event::Reset);
            self.spring.borrow_mut().snap();
            self.settle_window();
        }
        self.update_visibility();
    }

    // ── wing dodging ────────────────────────────────────────────────────────

    /// Which wings have something to show *and* room beside the notch.
    /// Which sides have something to show, ignoring occupancy. 0 when wings
    /// are off or cannot be safely checked.
    fn content_wing_mask(&self, snap: &Snapshot) -> u8 {
        let cfg = self.config.borrow();
        let media_on = cfg.media.show_in_wings;
        if !cfg.appearance.show_wings {
            return 0;
        }
        // Fail-safe: without Accessibility the menu bar cannot be checked, so
        // nothing may extend past the hardware notch.
        if cfg.appearance.dodge_menu_bar && !crate::accessibility::is_trusted() {
            if !self.ax_note_logged.replace(true) {
                tracing::info!(
                    "wings are hidden until Accessibility is granted: the menu bar beside the notch cannot be checked"
                );
            }
            return 0;
        }
        // A paused track keeps its wings only for a moment after it was playing.
        let current = snap.media.primary().filter(|_| media_on);
        let playing = current.filter(|n| n.is_playing());
        let recent_paused =
            current.filter(|n| !n.is_playing()).filter(|_| now_ms() - self.last_playing_ms.get() < PAUSED_WINGS_MS);
        let browser = snap.audio.playing_app().filter(|_| media_on && playing.is_none());
        if !snap.wants_wings_with(media_on) && recent_paused.is_none() {
            return 0;
        }
        if !cfg.appearance.dodge_menu_bar {
            return WING_LEFT | WING_RIGHT;
        }
        // Per-side content, mirroring views/collapsed.rs.
        let low = snap.battery.as_ref().is_some_and(|b| b.is_low());
        let call_wing = snap.active_call.is_some() || !snap.calls.is_empty();
        let left = call_wing
            || playing.is_some()
            || browser.is_some()
            || recent_paused.is_some()
            || low
            || snap.battery.as_ref().is_some_and(|b| b.present && b.charging)
            || !snap.tray.is_empty();
        let right = call_wing
            || snap.unread > 0
            || snap.focus.active == Some(true)
            || (snap.unread == 0 && (playing.is_some() || browser.is_some() || recent_paused.is_some()));
        let mut mask = 0;
        if left {
            mask |= WING_LEFT;
        }
        if right {
            mask |= WING_RIGHT;
        }
        mask
    }

    /// How long a chevron-popped wing stays out over the menu bar items.
    const WING_OVERRIDE_MS: i64 = 10_000;

    fn desired_wing_mask(&self, snap: &Snapshot) -> u8 {
        let cfg = self.config.borrow();
        let content = self.content_wing_mask(snap);
        if content == 0 || !cfg.appearance.dodge_menu_bar {
            self.wing_chevrons.set(0);
            self.wing_override.set((0, 0));
            return content;
        }
        let layout = self.layout.borrow();
        let base = wing_mask_for(&layout, content, &self.menu_bar_occupancy());
        // Sides with content but no room: the chevron offers them on demand,
        // the way the system's notch offers the items it hides.
        self.wing_chevrons.set(content & !base);
        let mut mask = base;
        let (forced, at) = self.wing_override.get();
        if forced != 0 {
            if notchforge_core::time::now_ms() - at < Self::WING_OVERRIDE_MS {
                mask |= forced & content;
            } else {
                self.wing_override.set((0, 0));
            }
        }
        mask
    }

    /// A wing chevron was clicked: pop that side's wing out over the menu bar
    /// items for a moment (the items are only ever covered by request, the
    /// way the system reveals the items its notch hides).
    pub fn wing_chevron_clicked(&self, sides: u8) {
        if self.wing_chevrons.get() & sides == 0 {
            return;
        }
        tracing::info!(sides, "wing chevron: popping the wing out over menu bar items");
        self.wing_override.set((sides, notchforge_core::time::now_ms()));
        self.refresh_wing_mask();
    }


    /// Is there room beside the notch on each side? Fail-safe: without
    /// Accessibility the spans *cannot* be checked, so they count as occupied —
    /// the wings stay hidden rather than risk covering menu bar items. Status
    /// Everything occupying the menu bar band of the island's screen: the
    /// frontmost app's menus, Control Center's status items, and any layer-25
    /// windows the list can see. Empty without Accessibility (callers fail
    /// safe). Coordinates are global.
    fn menu_bar_occupancy(&self) -> Vec<Rect> {
        if !crate::accessibility::is_trusted() {
            return Vec::new();
        }
        let primary_h = NSScreen::screens(self.mtm).iter().next().map_or(0.0, |s| s.frame().size.height);
        let mut occupied = crate::accessibility::app_menu_rects(primary_h);
        occupied.extend(crate::accessibility::status_item_rects(primary_h));
        occupied.extend(crate::windows::status_item_rects(&crate::windows::on_screen(self.mtm)));
        occupied
    }

    /// Where the floating pill should sit on its screen: centred unless menu
    /// bar items are in the way. Returns the offset from the notch's centre to
    /// feed the spring.
    fn update_pill_position(&self) {
        if !self.layout.borrow().notch.floating {
            return;
        }
        let offset = {
            let layout = self.layout.borrow();
            let frame = layout.screen.frame;
            let pill_w = layout.notch.rect.width;
            if !crate::accessibility::is_trusted() {
                0.0 // Nothing can be checked: stay centred.
            } else {
                let band_top = frame.max_y() - layout.screen.menu_bar_height();
                let occ: Vec<(f64, f64)> = self
                    .menu_bar_occupancy()
                    .into_iter()
                    // Only items of this screen's menu bar, in screen-local x.
                    .filter(|r| r.max_y() > band_top && r.y < frame.max_y())
                    .map(|r| (r.x - frame.x, r.max_x() - frame.x))
                    .collect();
                let x = notchforge_core::geometry::pill_x(frame.width, pill_w, &occ);
                // Offset from the pill's default (centred) position.
                (frame.x + x + pill_w / 2.0) - layout.notch.rect.mid_x()
            }
        };
        let mut px = self.pill_x.borrow_mut();
        if (px.target - offset).abs() > 0.5 {
            px.set_target(offset, notchforge_core::spring::SpringParams::with_response(0.45, 0.9));
            self.start_animation();
        }
    }

    /// Recompute the wing mask and tell the machine when it changed. Runs on
    /// every snapshot, and every 2 s while the wings are visible (menu bar
    /// items come and go).
    /// Keep the mirror islands in step with the screens, the config and the
    /// latest snapshot.
    fn sync_mirrors(&self, snap: &Snapshot) {
        let cfg = self.config.borrow().clone();
        let screens = screen::screens(self.mtm);
        let primary = self.layout.borrow().screen.id;
        let palette = self.palette.borrow().clone();
        self.mirrors.borrow_mut().sync(self.mtm, &screens, primary, &cfg, &palette);
        let count = self.mirrors.borrow().count();
        if count != self.mirror_count.get() {
            self.mirror_count.set(count);
            tracing::info!(mirrors = count, screens = screens.len(), primary, "mirror islands synced");
        }
        let content = self.content_wing_mask(snap);
        let opts = crate::views::collapsed::WingOptions {
            media: cfg.media.show_in_wings,
            animate: false,
            mask: content,
            chevrons: 0, // mirrors offer no chevrons; clicks move the island
        };
        let primary_h = NSScreen::screens(self.mtm).iter().next().map_or(0.0, |s| s.frame().size.height);
        let occupancy = self.menu_bar_occupancy();
        let decision = WingDecision { content_mask: content, opts, base_occupancy: &occupancy };
        self.mirrors.borrow_mut().update(self.mtm, snap, &self.palette.borrow(), decision, primary_h);
    }

    fn refresh_wing_mask(&self) {
        self.update_pill_position();
        let chevrons = self.wing_chevrons.get();
        let mask = self.desired_wing_mask(&self.snapshot.borrow());
        if mask != self.wing_mask.replace(mask) {
            tracing::info!(mask, "wing mask changed");
            self.handle(Event::WingsChanged(mask));
        } else if chevrons != self.wing_chevrons.replace(chevrons) {
            // Chevron appeared or vanished without a mask change: redraw.
            self.render_small(&self.snapshot.borrow());
        }
        let want_timer = mask != 0 && self.wing_timer.borrow().is_none();
        if want_timer {
            let block = RcBlock::new(|_t: NonNull<NSTimer>| {
                crate::app::with_controller(|c| c.refresh_wing_mask());
            });
            // SAFETY: documented NSTimer API; the block is copied by the timer.
            let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(2.0, true, &block) };
            // SAFETY: valid timer and mode.
            unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
            *self.wing_timer.borrow_mut() = Some(timer);
        } else if mask == 0 {
            if let Some(t) = self.wing_timer.borrow_mut().take() {
                t.invalidate();
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

    /// Which content container a layout renders in. Width-only differences
    /// (a wing appearing beside the notch) keep the same container.
    fn container_of(kind: LayoutKind) -> LayoutKind {
        match kind {
            LayoutKind::Collapsed | LayoutKind::CollapsedWithWings(_) => LayoutKind::Collapsed,
            other => other,
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
        let notch_h = self.layout.borrow().notch.rect.height;
        if shape.height > notch_h + 1.0 {
            self.glass_span.set(shape.height - notch_h);
            self.glass.set_open_panel(kind == LayoutKind::Expanded, self.high_contrast());
        }
        {
            let mut s = self.spring.borrow_mut();
            s.animate_to(shape.width, shape.height, shape.corner_radius, speed);
            // The content fades only when the panel actually changes shape —
            // a wing growing beside the notch keeps its content on screen.
            if Self::container_of(previous) != Self::container_of(kind) {
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
        self.pill_x.borrow_mut().step(dt);
        self.apply_frame();
        if self.spring.borrow().is_settled() && self.pill_x.borrow().is_settled() {
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
        let floating = self.layout.borrow().notch.floating;
        // The floating pill's sideways offset rides on the layout so zones,
        // hover math and the window all agree on where the island is.
        if floating {
            self.layout.borrow_mut().x_offset = self.pill_x.borrow().value;
        }
        let g = self.layout.borrow().island_global(w, h);
        let win = self.panel.frame();
        let radius = s.radius.value.clamp(0.0, h / 2.0);
        // A physical notch extends one radius above the window's top edge so
        // the window clips its rounded top corners away and the island merges
        // with the bezel. The floating capsule stays whole: every corner is
        // rounded, nothing touches the screen's top edge.
        let overhang = if floating { 0.0 } else { radius.ceil() };
        self.island.setFrame(rect(g.x - win.origin.x, g.y - win.origin.y, w, h + overhang));
        if let Some(layer) = self.island.layer() {
            layer.setCornerRadius(radius);
        }
        let notch_h = self.layout.borrow().notch.rect.height;
        let t = notchforge_core::geometry::glass_progress(h, notch_h, self.glass_span.get());
        self.glass.apply(w, h, overhang, radius, t);
        if floating {
            // No bezel to flare into: the flares stay hidden.
            crate::ui::without_animation(|| {
                self.ears[0].setHidden(true);
                self.ears[1].setHidden(true);
            });
        } else {
            self.place_ears(g.x - win.origin.x, w, win.size.height, radius);
        }
        let views = self.views.borrow();
        let c = views.container(self.kind.get());
        let cs = c.frame().size;
        // The collapsed view is always built at full wing width; with a wing
        // dodged the island is narrower and clips it. Skip the hidden left
        // wing's space so the notch area of the view stays under the island.
        let ox = match self.kind.get() {
            LayoutKind::CollapsedWithWings(mask) if mask & WING_LEFT == 0 && cs.width > w => -(cs.width - w),
            _ => (w - cs.width) / 2.0,
        };
        c.setFrameOrigin(NSPoint::new(ox, h - cs.height));
        c.setAlphaValue(s.content_alpha.value.clamp(0.0, 1.0));
    }

    fn high_contrast(&self) -> bool {
        self.config.borrow().appearance.high_contrast || system_increase_contrast()
    }

    /// Put the flares against the island's left and right edges at the top of
    /// the window. `x`/`w` are the island's window-local extent.
    fn place_ears(&self, x: f64, w: f64, window_h: f64, corner_radius: f64) {
        let e = notchforge_core::geometry::notch_ear_radius(corner_radius);
        let changed = (self.ear_radius.get() - e).abs() > 0.01;
        crate::ui::without_animation(|| {
            self.ears[0].setFrame(rect(x - e, window_h - e, e, e));
            self.ears[1].setFrame(rect(x + w, window_h - e, e, e));
            if changed {
                crate::ui::set_notch_flare(&self.ears[0], e, true);
                crate::ui::set_notch_flare(&self.ears[1], e, false);
            }
        });
        self.ear_radius.set(e);
    }

    /// Shrink the window to the island (plus room for the flares) so little
    /// outside it intercepts clicks.
    fn settle_window(&self) {
        let s = *self.spring.borrow();
        let g = self.layout.borrow().island_global(s.width.value, s.height.value);
        // The flares need room beside a physical notch; the floating capsule
        // only needs a pixel or two so its rounded corners are not clipped.
        let ear = if self.layout.borrow().notch.floating { 2.0 } else { notchforge_core::geometry::NOTCH_EAR_MAX };
        let win = notchforge_core::geometry::Rect::new(g.x - ear, g.y, g.width + 2.0 * ear, g.height);
        self.panel.setFrame_display(screen::to_nsrect(win), true);
        self.root.setFrame(rect(0.0, 0.0, win.width, win.height));
        self.apply_frame();
    }

    /// Screen configuration changed: recompute geometry and rebuild views.
    pub fn relayout(&self) {
        let cfg = self.config.borrow().clone();
        let Some(layout) = current_layout_pinned(self.mtm, &cfg, self.focus_override.get()) else { return };
        // The animated pill offset rides on the layout; it must not count as
        // a screen change (it would rebuild the views mid-glide).
        let same = {
            let old = self.layout.borrow();
            old.screen == layout.screen && old.notch == layout.notch && old.appearance == layout.appearance
        };
        if same {
            return;
        }
        tracing::info!(screen = layout.screen.id, notch = layout.notch.physical, "screen layout changed");
        self.pill_x.borrow_mut().snap();
        self.panel.setHasShadow(layout.notch.floating);
        self.glass.set_notch_height(layout.notch.rect.height);
        self.sync_mirrors(&self.snapshot.borrow());
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
        if let Some(l) = self.island.layer() {
            crate::glass::smooth_edges(&l);
        }
        let snap = self.snapshot.borrow().clone();
        self.render_small(&snap);
    }

    // ── input ────────────────────────────────────────────────────────────

    fn global_event(&self, event: &NSEvent) {
        // A pass-through click's synthetic down/up passes through here too.
        if self.repost_click.get() {
            return;
        }
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
        // Hidden for fullscreen: no hover, no peek, nothing to point at.
        if !self.panel.isVisible() {
            return;
        }
        let p = Point { x: p.x, y: p.y };
        // Hovering the notch brings hidden calls back.
        if self.calls_hidden.get() && self.layout.borrow().activation_zone().contains_inclusive(p) {
            self.calls_hidden.set(false);
            let snap = self.snapshot.borrow().clone();
            self.sync_calls(&snap);
        }
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
            self.click_through(global, false);
        }
    }

    /// Should a click at `global` fall through to whatever is under the
    /// window? True in the window's margins outside the island (ears, and the
    /// bigger frame while animating), and — while a glance is open — in the
    /// menu-bar strip beside the notch, where no control of ours lives. The
    /// pinned panel keeps its whole top row interactive.
    fn passes_through(&self, global: Point, island: Rect) -> bool {
        if !island.contains_inclusive(global) {
            return true;
        }
        let notch = self.layout.borrow().notch.rect;
        let beside_notch = global.x < notch.x || global.x > notch.max_x();
        let in_band = global.y <= island.y + notch.height;
        match self.kind.get() {
            LayoutKind::Peek | LayoutKind::Compact | LayoutKind::Incoming(_) => in_band && beside_notch,
            LayoutKind::Collapsed | LayoutKind::CollapsedWithWings(_) | LayoutKind::Expanded => false,
        }
    }

    /// A click landed somewhere we don't own: collapse, then give the click to
    /// whatever app is actually under the pointer. Our window stops
    /// intercepting events first, so the synthetic click skips it.
    fn click_through(&self, global: Point, right: bool) {
        self.handle(Event::ClickOutside);
        if self.calls_showing.get() {
            // The call card owns the window until it stops ringing; trying to
            // hand the click through would only click into it.
            return;
        }
        self.panel.setIgnoresMouseEvents(true);
        self.post_click(global, right);
    }

    /// Post a synthetic click at an AppKit global point (CG wants top-left
    /// coordinates). The mouse-up follows a beat later; until then the global
    /// monitor ignores our own events.
    fn post_click(&self, global: Point, right: bool) {
        use crate::ffi::cg;
        let primary_h = NSScreen::screens(self.mtm).iter().next().map_or(0.0, |s| s.frame().size.height);
        let pt = cg::CGPoint { x: global.x, y: primary_h - global.y };
        let (down, up, button) = if right {
            (cg::kCGEventRightMouseDown, cg::kCGEventRightMouseUp, cg::kCGMouseButtonRight)
        } else {
            (cg::kCGEventLeftMouseDown, cg::kCGEventLeftMouseUp, cg::kCGMouseButtonLeft)
        };
        self.repost_click.set(true);
        // SAFETY: CGEvent create/post with valid arguments; cf::Owned releases.
        unsafe {
            if let Some(ev) =
                crate::ffi::cf::Owned::from_create(cg::CGEventCreateMouseEvent(std::ptr::null(), down, pt, button))
            {
                cg::CGEventPost(cg::kCGHIDEventTap, ev.as_ptr());
            }
        }
        let when = dispatch2::DispatchTime::NOW.time(30_000_000);
        let _ = dispatch2::DispatchQueue::main().after(when, move || {
            // SAFETY: as above.
            unsafe {
                if let Some(ev) =
                    crate::ffi::cf::Owned::from_create(cg::CGEventCreateMouseEvent(std::ptr::null(), up, pt, button))
                {
                    cg::CGEventPost(cg::kCGHIDEventTap, ev.as_ptr());
                }
            }
            crate::app::with_controller(|c| {
                c.repost_click.set(false);
                // Back to whatever interactivity applies now (the quick media
                // toggle keeps the collapsed island clickable).
                c.update_interactive();
            });
        });
    }

    /// Called by the panel for every left mouse-down. Returns `true` when the
    /// click was handled here (pinning a hover/peek island, or passing it
    /// through to the app underneath).
    pub fn intercept_mouse_down(&self, window_point: NSPoint) -> bool {
        if self.repost_click.get() {
            return false;
        }
        let win = self.panel.frame();
        let global = Point { x: win.origin.x + window_point.x, y: win.origin.y + window_point.y };
        let island = {
            let s = *self.spring.borrow();
            self.layout.borrow().island_global(s.width.value, s.height.value)
        };
        if matches!(self.machine.borrow().state(), State::Expanded | State::DropTarget { .. }) {
            if self.passes_through(global, island) {
                self.click_through(global, false);
                return true;
            }
            return false;
        }
        // A collapsed wing chevron pops its dodged wing out over the items.
        if matches!(self.kind.get(), LayoutKind::Collapsed | LayoutKind::CollapsedWithWings(_)) {
            if let Some(sides) = self.views.borrow().collapsed.chevron_hit(window_point) {
                self.wing_chevron_clicked(sides);
                return true;
            }
        }
        // Quick access: a collapsed wing showing the playing audio toggles it.
        // Any other click on the collapsed island falls through to the menu
        // bar item it covers, as it always has.
        if self.media_wing_clickable() {
            if self.views.borrow().collapsed.media_hit(window_point) {
                self.media_wing_clicked();
            } else {
                self.click_through(global, false);
            }
            return true;
        }
        // The peek's button opens the notification itself, or answers its
        // authorization question (Allow / Deny / Cancel).
        if self.kind.get() == LayoutKind::Peek {
            let hit = self.views.borrow().peek.button_hit(window_point);
            if let Some(kind) = hit {
                let dismiss = self.views.borrow().peek.press(kind);
                if dismiss {
                    self.handle(Event::ClickOutside);
                }
                return true;
            }
        }
        // Clicking the hover card's "now playing" opens straight on the music page.
        if self.kind.get() == LayoutKind::Compact && self.views.borrow().compact.media_hit(window_point) {
            self.views.borrow().expanded.show_tab(crate::views::expanded::Tab::Media);
        }
        if self.passes_through(global, island) {
            self.click_through(global, false);
            return true;
        }
        self.root_mouse_down(window_point);
        true
    }

    /// Right-click on the island. While the collapsed island is only
    /// incidentally interactive (quick media toggle), right-clicks keep
    /// passing through to whatever menu bar item is underneath.
    pub fn right_mouse_down(&self, window_point: NSPoint) {
        if self.calls_showing.get() {
            return;
        }
        if matches!(self.machine.borrow().state(), State::Collapsed | State::Arming) {
            let win = self.panel.frame();
            let global = Point { x: win.origin.x + window_point.x, y: win.origin.y + window_point.y };
            self.click_through(global, true);
        }
    }

    /// Switch the Agents page's time range.
    pub fn show_usage_range(&self, range: usize) {
        self.views.borrow().expanded.usage.set_range(range);
    }

    // ── media page ───────────────────────────────────────────────────────

    /// The resolution pill was clicked: offer the display's modes.
    /// Flip the microphone mute from the expanded panel's toggle.
    pub fn toggle_mic(&self) {
        if let Some(muted) = crate::controls::mic::toggle() {
            self.views.borrow().expanded.set_mic_muted(Some(muted));
        }
    }

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
        views.expanded.preview_controls(0.72, 0.62, "1512 × 982");
        views.expanded.show_tab(Tab::Overview);
        show("07a-notched-overview", LayoutKind::Expanded, &views.expanded.root);
        views.expanded.media.invalidate();
        views.expanded.media.set_active_playlist(music.media.playlists.first().map(|p| p.id.clone()));
        views.expanded.media.update(music);
        views.expanded.show_tab(Tab::Media);
        show("07b-notched-music", LayoutKind::Expanded, &views.expanded.root);
        views.compact.update(music, &palette);
        show("07c-notched-hover", LayoutKind::Compact, &views.compact.root);
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
            crate::views::collapsed::WingOptions { media: true, animate: false, mask: WING_LEFT | WING_RIGHT, chevrons: 0 },
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
        let mut hovering = music.clone();
        hovering.active_call = Some(on_call.clone());
        views.compact.update(&hovering, &palette);
        show("07n-notched-hover-in-call", LayoutKind::Compact, &views.compact.root);
        views.compact.update(music, &palette);
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
            crate::views::collapsed::WingOptions { media: true, animate: false, mask: WING_LEFT | WING_RIGHT, chevrons: 0 },
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
    /// The sources bar pinned a player: the music page follows it.
    pub fn media_pin_source(&self, player: MediaPlayer) {
        self.views.borrow().expanded.show_tab(crate::views::expanded::Tab::Media);
        self.views.borrow().expanded.media.pin(player);
    }

    pub fn media_rerender(&self) {
        let snap = self.snapshot.borrow().clone();
        let views = self.views.borrow();
        views.expanded.media.invalidate();
        views.expanded.media.update(&snap);
    }

    pub fn key_down(&self, key: u16, flags: NSEventModifierFlags) -> bool {
        match key {
            KEY_ESCAPE if self.reply_to.borrow().is_some() => {
                self.cancel_reply();
                true
            }
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

    /// ⌃⌥C (or the menu item): shelve the clipboard's text or link into the
    /// tray, with a small confirmation peek.
    pub fn capture_clipboard(&self) {
        use notchforge_core::apps::AppRef;
        use notchforge_core::model::{NewNotification, NotificationKind, Origin, Priority};
        let Some(text) = crate::sharing::pasteboard_string().filter(|t| !t.trim().is_empty()) else {
            tracing::debug!("⌃⌥C with an empty clipboard");
            return;
        };
        let preview = text.lines().next().unwrap_or("").chars().take(64).collect::<String>();
        self.engine.send(Command::TrayAddText(Box::new(text)));
        let n = NewNotification {
            app: AppRef { known: None, bundle_id: None, display_name: "Tray".into(), platform: None },
            origin: Origin::Internal,
            kind: NotificationKind::Info,
            priority: Some(Priority::Normal),
            title: "Shelved".into(),
            subtitle: None,
            body: preview,
            timestamp_ms: now_ms(),
            thread: Some("shelved".into()),
            url: None,
            tokens: None,
        };
        self.engine.send(Command::Inject(Box::new(n)));
    }

    // ── public actions (menu bar, buttons) ───────────────────────────────

    pub fn toggle(&self) {
        self.handle(Event::Toggle);
    }

    pub fn collapse(&self) {
        self.handle(Event::Escape);
    }

    /// Start a quick reply: expand the native banner's reply field (while it
    /// is on screen), pin the panel and open the reply bar in the expanded
    /// panel. Called from the peek and notification-row Reply buttons.
    pub fn start_reply(&self, id: notchforge_core::model::NotificationId) {
        let Some(n) = self.snapshot.borrow().notifications.iter().find(|n| n.id == id).cloned() else { return };
        let pressed = crate::accessibility::press_banner_reply(&n.title);
        tracing::info!(pressed, app = %n.app.display_name, "reply started");
        if !matches!(self.machine.borrow().state(), State::Expanded) {
            self.handle(Event::Click); // peek/hover pins the panel
        }
        *self.reply_to.borrow_mut() = Some(n.clone());
        self.views.borrow().expanded.show_reply_bar(&n);
        let field = self.views.borrow().expanded.reply_field();
        self.panel.makeFirstResponder(Some(&field));
    }

    /// Deliver the typed reply through the native banner's own field. When the
    /// banner is gone, fall back to bringing the app forward.
    pub fn send_reply(&self) {
        let Some(n) = self.reply_to.borrow_mut().take() else { return };
        self.views.borrow().expanded.hide_reply_bar();
        let text = self.views.borrow().expanded.reply_text().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let delivered = crate::accessibility::fill_banner_reply(&n.title, &text);
        tracing::info!(delivered, app = %n.app.display_name, len = text.len(), "reply sent");
        if !delivered {
            // The banner expired while typing: the app itself is the fallback.
            if let Some(b) = n.app.bundle_id.as_deref() {
                crate::sharing::open_app(b);
            }
        }
        let field = self.views.borrow().expanded.reply_field();
        self.panel.makeFirstResponder(Some(&self.root));
        let _ = field;
    }

    /// Close the reply bar without sending.
    pub fn cancel_reply(&self) {
        if self.reply_to.borrow().is_some() {
            *self.reply_to.borrow_mut() = None;
            self.views.borrow().expanded.hide_reply_bar();
        }
    }

    /// Answer an authorization prompt: Allow / Deny for an agent asking to show
    /// notifications, Cancel for a Spotify sign-in waiting on the browser.
    /// Add a plan to Reminders. EventKit runs on the main thread, which this
    /// is, so the save waits on a worker that hops back to it.
    pub fn add_plan(&self, title: &str) {
        self.views.borrow().expanded.planner.plan_added(title);
        let title = title.to_owned();
        std::thread::spawn(move || {
            if !notchforge_providers::calendar::add_reminder(&title) {
                tracing::warn!("could not add a reminder");
            }
        });
    }

    /// Tick a plan off in Reminders.
    pub fn complete_plan(&self, id: &str) {
        self.views.borrow().expanded.planner.plan_done(id);
        let id = id.to_owned();
        std::thread::spawn(move || {
            notchforge_providers::calendar::complete_reminder_by_id(&id);
        });
    }

    pub fn new_note(&self) {
        self.views.borrow().expanded.planner.new_note();
    }

    pub fn open_note(&self, id: u64) {
        self.views.borrow().expanded.planner.open_note(id);
    }

    pub fn answer_auth(&self, target: &notchforge_core::model::AuthTarget, primary: bool) {
        use notchforge_core::model::AuthTarget;
        match target {
            AuthTarget::Agent(key) => self.engine.send(Command::Authorize { key: key.clone(), allow: primary }),
            AuthTarget::Spotify => {
                self.engine.send(Command::Provider(notchforge_core::provider::ProviderCommand::SpotifyCancel))
            }
        }
    }

    /// AppKit reported the panel resigning key. Close only if it really is no
    /// longer key (opening the panel makes it key, which can report a resign
    /// of the previous key window first).
    pub fn panel_lost_key(&self) {
        if !self.panel.isKeyWindow() {
            self.panel_resigned_key();
        }
    }

    /// The panel lost key status: if it was pinned open, close it. (A modal
    /// the island itself raised, such as the resolution confirmation, also
    /// takes key; closing then is fine, the answer still applies.)
    pub fn panel_resigned_key(&self) {
        let pinned = self.machine.try_borrow().map(|m| matches!(m.state(), State::Expanded)).unwrap_or(false);
        if pinned {
            self.handle(Event::ClickOutside);
        }
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

    /// Show, resize or stop showing the ringing calls.
    fn sync_calls(&self, snap: &Snapshot) {
        let ids: Vec<_> = snap.calls.iter().map(|c| c.id).collect();
        let is_new = ids.iter().any(|id| !self.known_calls.borrow().contains(id));
        *self.known_calls.borrow_mut() = ids;
        if is_new || (snap.calls.is_empty() && snap.active_call.is_none()) {
            self.calls_hidden.set(false);
        }
        self.set_call_timer(!snap.calls.is_empty() || snap.active_call.is_some());
        self.views.borrow().expanded.set_call(snap.active_call.as_ref(), now_ms());
        let show = !snap.calls.is_empty() && !self.calls_hidden.get();
        // Just picked up: show who with and how to hang up, then settle beside the notch.
        let just_answered = snap.active_call.as_ref().filter(|a| {
            snap.calls.is_empty() && !self.calls_hidden.get() && now_ms() - a.started_ms < JUST_ANSWERED_MS
        });
        if let Some(call) = just_answered {
            self.views.borrow().call.update_active(call, now_ms());
            self.calls_showing.set(true);
            self.update_interactive();
            if self.kind.get() != LayoutKind::Incoming(1) {
                self.set_layout(LayoutKind::Incoming(1));
            }
        } else if show {
            let shown = snap.calls.len().min(notchforge_core::geometry::MAX_VISIBLE_CALLS) as u8;
            self.views.borrow().call.update(&snap.calls, !self.reduce_motion());
            self.calls_showing.set(true);
            self.update_interactive();
            if self.kind.get() != LayoutKind::Incoming(shown) {
                self.set_layout(LayoutKind::Incoming(shown));
            }
        } else if self.calls_showing.replace(false) {
            self.update_interactive();
            self.set_layout(self.wanted_kind.get());
        }
    }

    /// Check once a second, while anything rings, for calls that stopped.
    fn set_call_timer(&self, on: bool) {
        let mut slot = self.call_timer.borrow_mut();
        if on && slot.is_none() {
            let block = RcBlock::new(|_t: NonNull<NSTimer>| {
                crate::app::send(Command::ExpireCalls);
                crate::app::with_controller(|c| c.call_second());
            });
            // SAFETY: documented NSTimer API; the block is copied by the timer.
            let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(1.0, true, &block) };
            // SAFETY: valid timer and run loop mode constant.
            unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
            *slot = Some(timer);
        } else if !on {
            if let Some(t) = slot.take() {
                t.invalidate();
            }
        }
    }

    /// Once a second during a call: move the clocks on, and settle the view.
    pub fn call_second(&self) {
        let snap = self.snapshot.borrow().clone();
        let Some(call) = &snap.active_call else { return };
        let now = now_ms();
        self.views.borrow().call.set_clock(&notchforge_core::calls::call_clock(call.started_ms, now));
        self.render_small(&snap);
        // Rebuilding the big view each second would lose a click in progress.
        if now - call.started_ms >= JUST_ANSWERED_MS || !snap.calls.is_empty() {
            self.sync_calls(&snap);
        }
    }

    /// The process whose windows a call's buttons are in. A call seen through
    /// a notification has none yet, so its app's main process is the place to look.
    fn call_pid(window_pid: Option<i32>, app: &notchforge_core::apps::AppRef) -> Option<i32> {
        window_pid.or_else(|| {
            let pid = crate::call_watch::main_pid(app.bundle_id.as_deref()?, 0);
            (pid != 0).then_some(pid)
        })
    }

    /// The process a call is ringing in right now. A call seen only through a
    /// notification may have no buttons yet, and the app's other "Accept" and
    /// "Decline" buttons are not this call's.
    fn ringing_pid(call: &notchforge_core::calls::Call) -> Option<i32> {
        let pid = Self::call_pid(call.window_pid, &call.app)?;
        (call.window_pid.is_some() || crate::accessibility::inspect_call_window(pid, true).ringing).then_some(pid)
    }

    /// Press whatever ends the call: its hang-up button, or its decline button
    /// if it turns out to be ringing still.
    fn press_hang_up(call: &notchforge_core::calls::ActiveCall) -> bool {
        use notchforge_core::calls::{DECLINE_WORDS, HANG_UP_WORDS, LEAVE_WORDS};
        Self::call_pid(call.window_pid, &call.app).is_some_and(|pid| {
            crate::accessibility::press_window_button(pid, HANG_UP_WORDS)
                || crate::accessibility::press_window_button(pid, LEAVE_WORDS)
                || (crate::accessibility::inspect_call_window(pid, true).ringing
                    && crate::accessibility::press_window_button(pid, DECLINE_WORDS))
        })
    }

    /// End the call in progress with the app's own button. If there's none to
    /// press, its tab or app may be out of view: bring it forward and try once
    /// more. Pressing hang up again means it's over, whatever the app shows.
    pub fn hang_up(&self) {
        let snap = self.snapshot.borrow().clone();
        let Some(call) = &snap.active_call else { return };
        let pressed = Self::press_hang_up(call);
        let now = now_ms();
        let again = now - self.hang_up_tried_ms.get() < HANG_UP_AGAIN_MS;
        tracing::debug!(pressed, again, source = %call.source, "call: hang up");
        let bundle = call.app.bundle_id.clone();
        match bundle {
            Some(b) if !pressed && !again => {
                self.hang_up_tried_ms.set(now);
                crate::call_watch::bring_forward(b, call.caller.clone(), call.source.clone(), None);
                let when = dispatch2::DispatchTime::NOW.time(1_500_000_000);
                let _ = dispatch2::DispatchQueue::main().after(when, || {
                    crate::app::with_controller(|c| c.hang_up_in_view());
                });
            }
            _ => {
                self.hang_up_tried_ms.set(0);
                self.engine.send(Command::EndActiveCall);
            }
        }
    }

    /// Hang up again, now the call's tab or app has come forward.
    fn hang_up_in_view(&self) {
        let snap = self.snapshot.borrow().clone();
        let Some(call) = &snap.active_call else { return };
        if Self::press_hang_up(call) {
            tracing::debug!(source = %call.source, "call: hung up once in view");
            self.hang_up_tried_ms.set(0);
            self.engine.send(Command::EndActiveCall);
        }
    }

    /// Answer, decline or reorder a call.
    pub fn call_action(&self, call: &notchforge_core::calls::Call, action: crate::views::call::CallAction) {
        use crate::views::call::CallAction;
        use notchforge_core::calls::{ANSWER_WORDS, DECLINE_WORDS};
        match action {
            CallAction::Promote => self.engine.send(Command::PromoteCall(call.id)),
            CallAction::Answer => {
                let pressed = Self::ringing_pid(call)
                    .is_some_and(|pid| crate::accessibility::press_window_button(pid, ANSWER_WORDS))
                    || crate::accessibility::press_banner_button(&call.banner_title, ANSWER_WORDS);
                tracing::debug!(pressed, source = %call.source, "call: answer");
                if pressed {
                    self.engine.send(Command::CallAnswered(call.id));
                    return;
                }
                // Still ringing out of view: bring it forward, and the watch
                // presses its button as soon as it can be seen.
                if !crate::accessibility::open_banner(&call.banner_title) {
                    if let Some(b) = call.app.bundle_id.clone() {
                        let (caller, source) = (call.caller.clone(), call.source.clone());
                        crate::call_watch::bring_forward(b, caller, source, call.url.clone());
                    }
                }
                self.engine.send(Command::CallAnswering(call.id));
            }
            CallAction::Decline => {
                // Without a button to press, this only stops the notch ringing.
                let pressed = Self::ringing_pid(call)
                    .is_some_and(|pid| crate::accessibility::press_window_button(pid, DECLINE_WORDS))
                    || crate::accessibility::press_banner_button(&call.banner_title, DECLINE_WORDS);
                tracing::debug!(pressed, source = %call.source, "call: decline");
                self.engine.send(Command::EndCall(call.id));
            }
        }
    }

    /// Move the ringing calls out of the way until something new rings.
    pub fn hide_calls(&self) {
        self.calls_hidden.set(true);
        let snap = self.snapshot.borrow().clone();
        self.sync_calls(&snap);
    }

    pub fn apply_snapshot(&self, snap: Arc<Snapshot>) {
        *self.snapshot.borrow_mut() = snap.clone();
        if snap.media.is_playing() || snap.audio.is_playing() {
            self.last_playing_ms.set(now_ms());
        }
        self.sync_calls(&snap);
        self.render_small(&snap);
        self.refresh_wing_mask();
        self.sync_mirrors(&snap);
        // Media may have started or stopped: the collapsed wings' clickability
        // follows what the wings are showing.
        self.update_interactive();
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
            mask: self.wing_mask.get(),
            chevrons: self.wing_chevrons.get(),
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
        if self.machine.borrow().is_open() || !self.panel.isVisible() {
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
            // The user's own screen choice wins again.
            self.focus_override.set(None);
            // Force a geometry + view rebuild.
            if let Some(layout) = current_layout_pinned(self.mtm, &cfg, self.focus_override.get()) {
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
        if old.general.hide_in_fullscreen != cfg.general.hide_in_fullscreen {
            // Turning hiding on or off while a fullscreen app is up applies
            // right away; unchanged states are left alone.
            self.apply_fullscreen_state();
        }
        // The screen may have moved to another display: re-read both watchers.
        self.evaluate_fullscreen();
        self.refresh_wing_mask();
        self.sync_mirrors(&self.snapshot.borrow());
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

    /// The window's whole layer tree (island and flares) with its size.
    pub fn window_layer(&self) -> Option<(Retained<objc2_quartz_core::CALayer>, f64, f64)> {
        let size = self.root.frame().size;
        self.root.layer().map(|l| (l, size.width, size.height))
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

    /// The configuration currently in force (self-test & diagnostics).
    pub fn current_config(&self) -> Arc<Config> {
        self.config.borrow().clone()
    }

    pub fn screen_id(&self) -> u32 {
        self.layout.borrow().screen.id
    }

    /// A mirror was clicked: move the primary island to that screen. The pin
    /// lasts until the screen disappears (or the user changes the screen
    /// choice, which resets it).
    pub fn focus_screen(&self, id: u32) {
        if self.screen_id() == id {
            return; // already the primary
        }
        tracing::info!(screen = id, "mirror clicked: moving the island to that screen");
        self.focus_override.set(Some(id));
        self.relayout();
    }

    /// Set the wing chevron mask directly, bypassing occupancy detection
    /// (self-test).
    pub fn force_wing_chevrons(&self, sides: u8) {
        self.wing_chevrons.set(sides);
        self.render_small(&self.snapshot.borrow());
    }

    /// Is a wing chevron visible (self-test)?
    pub fn wing_chevron_showing(&self, sides: u8) -> bool {
        self.views.borrow().collapsed.chevron_showing(sides)
    }

    /// Send a wing mask directly, bypassing occupancy detection (self-test).
    pub fn force_wing_mask(&self, mask: u8) {
        self.wing_mask.set(mask);
        self.handle(Event::WingsChanged(mask));
        // Settle at once: the self-test asserts the island size right after.
        self.spring.borrow_mut().snap();
        self.settle_window();
    }

    /// Whether the fullscreen watcher currently sees a covering app.
    pub fn fullscreen_active(&self) -> bool {
        self.fs_active.get()
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
        self.mirrors.borrow_mut().shutdown();
        self.panel.orderOut(None);
        let _ = NSApplication::sharedApplication(self.mtm);
    }
}
