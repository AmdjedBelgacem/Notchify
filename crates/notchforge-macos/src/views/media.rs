//! "Now Playing" page of the expanded panel.
//!
//! ```text
//! ┌──────────────────────────────────────────────┬─────────────────────────┐
//! │ ┌────────┐  ● Spotify                        │ Playlists  [Spotify|Music]
//! │ │  art   │  Teardrop                         │ ┌─────────────────────┐ │
//! │ │        │  Massive Attack — Mezzanine       │ │ ▣ Chill      34 ▶  │ │
//! │ └────────┘  ━━━━━━━━○──────────  1:23  -4:10  │ │ ▣ Road trip  80 ▶  │ │
//! │             ⇄   ⏮   ⏯   ⏭   ↻                │ │ …                   │ │
//! │ ┌ 🎧 AirPods Pro            [Output ▾] ┐      │ │                     │ │
//! │ │ Bluetooth · 48 kHz · 24-bit · Stereo │      │ │                     │ │
//! │ │ 🔈 ━━━━━━━━━━━○─────                  │      │ │                     │ │
//! │ └──────────────────────────────────────┘      │ │                     │ │
//! │ Playing audio                                 │ │                     │ │
//! │ ◉ Spotify → AirPods Pro   ◉ Safari → AirPods │ └─────────────────────┘ │
//! └──────────────────────────────────────────────┴─────────────────────────┘
//! ```
//!
//! The page shows the *featured* source: the playing Spotify/Music player,
//! else another app producing audio (controlled with media keys), else an
//! empty state with shortcuts to open a player. The backdrop is tinted with
//! the cover's average colour.

use crate::icons;
use crate::media::{self as glue, MediaKey};
use crate::ui::{
    FlippedView, Palette, Targets, Weight, click_overlay, font, label, make_card, mono_digits, rect,
    remove_all_subviews, rgb8, rgba, set_accessibility_label, set_button_symbol, set_symbol, set_text, symbol_button,
    symbol_view, text_button, wrapping_label,
};
use notchforge_core::engine::Snapshot;
use notchforge_core::model::{
    AudioState, MediaAction, MediaPlayer, MediaState, NowPlaying, PlaybackState, Playlist, PlaylistStatus,
};
use notchforge_core::provider::ProviderCommand;
use notchforge_core::time::now_ms;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSButton, NSControlSize, NSImageScaling, NSImageView, NSPopUpButton, NSScrollView, NSScrollerStyle,
    NSSegmentSwitchTracking, NSSegmentedControl, NSSlider, NSTextAlignment, NSTextField, NSView,
};
use objc2_foundation::{NSArray, NSString};
use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;

const PAD: f64 = 16.0;
const GAP: f64 = 14.0;
const ART: f64 = 120.0;
const ROW_H: f64 = 44.0;

/// What the left half is showing.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Featured {
    Player(MediaPlayer),
    /// Another app producing audio, controlled with media keys.
    App {
        name: String,
        bundle_id: Option<String>,
        app_path: Option<String>,
    },
    #[default]
    Nothing,
}

/// Pick the featured source from a snapshot.
pub fn featured(media: &MediaState, audio: &AudioState) -> Featured {
    if let Some(p) = media.primary() {
        return Featured::Player(p.player);
    }
    let rich = |b: &Option<String>| b.as_deref().and_then(MediaPlayer::from_bundle_id).is_some();
    if let Some(s) = audio.sources.iter().find(|s| !rich(&s.bundle_id)) {
        return Featured::App { name: s.name.clone(), bundle_id: s.bundle_id.clone(), app_path: s.app_path.clone() };
    }
    Featured::Nothing
}

/// `75.4` → `"1:15"`, `3700` → `"1:01:40"`.
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

type Shared = Rc<RefCell<PageState>>;
type TransportFn = Box<dyn Fn(&Shared)>;

#[derive(Default)]
struct PageState {
    featured: Featured,
    now: Option<NowPlaying>,
    /// Device UIDs in popup order.
    device_uids: Vec<String>,
    /// Player whose playlists are listed (user choice, else automatic).
    list_player: Option<MediaPlayer>,
    chosen_list_player: Option<MediaPlayer>,
    segment_players: Vec<MediaPlayer>,
    art_path: Option<PathBuf>,
    playlists_key: u64,
    sources_key: u64,
    media: MediaState,
}

pub struct MediaPage {
    pub root: Retained<FlippedView>,
    mtm: MainThreadMarker,
    p: Palette,
    _targets: Targets,
    row_targets: Targets,
    source_targets: Targets,
    state: Rc<RefCell<PageState>>,
    gradient: Retained<objc2_quartz_core::CAGradientLayer>,

    art: Retained<NSImageView>,
    art_placeholder: Retained<NSImageView>,
    badge_icon: Retained<NSImageView>,
    badge: Retained<NSTextField>,
    title: Retained<NSTextField>,
    subtitle: Retained<NSTextField>,
    progress: Retained<NSSlider>,
    elapsed: Retained<NSTextField>,
    remaining: Retained<NSTextField>,
    shuffle: Retained<NSButton>,
    prev: Retained<NSButton>,
    play: Retained<NSButton>,
    next: Retained<NSButton>,
    repeat: Retained<NSButton>,
    open_spotify: Retained<NSButton>,
    open_music: Retained<NSButton>,

    output_icon: Retained<NSImageView>,
    output_name: Retained<NSTextField>,
    output_caps: Retained<NSTextField>,
    output_popup: Retained<NSPopUpButton>,
    volume: Retained<NSSlider>,

    sources_header: Retained<NSTextField>,
    sources: Retained<FlippedView>,
    sources_width: f64,

    list_switch: Retained<NSSegmentedControl>,
    list_scroll: Retained<NSScrollView>,
    list_doc: Retained<FlippedView>,
    list_width: f64,
    list_message: Retained<NSTextField>,
    list_action: Retained<NSButton>,
    list_action_kind: Rc<Cell<ListAction>>,

    ticker: RefCell<Option<Retained<objc2_foundation::NSTimer>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ListAction {
    #[default]
    None,
    ConnectSpotify,
    OpenSettings,
    AllowAutomation,
    OpenMusic,
    Retry,
}

fn mouse_down() -> bool {
    objc2_app_kit::NSEvent::pressedMouseButtons() & 1 != 0
}

fn slider(mtm: MainThreadMarker, t: &crate::ui::ActionTarget, frame: objc2_foundation::NSRect) -> Retained<NSSlider> {
    // SAFETY: target kept alive by the page's `Targets`.
    let s = unsafe {
        NSSlider::sliderWithValue_minValue_maxValue_target_action(
            0.0,
            0.0,
            1.0,
            Some(t.as_any()),
            Some(crate::ui::ActionTarget::action()),
            mtm,
        )
    };
    s.setControlSize(NSControlSize::Small);
    s.setFrame(frame);
    s
}

impl MediaPage {
    pub fn new(mtm: MainThreadMarker, w: f64, h: f64, top: f64, p: &Palette) -> Self {
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        let targets = Targets::default();
        let state: Rc<RefCell<PageState>> = Rc::default();

        // ── tinted backdrop ──
        root.setWantsLayer(true);
        let gradient = objc2_quartz_core::CAGradientLayer::layer();
        gradient.setFrame(rect(0.0, 0.0, w, h));
        gradient.setStartPoint(objc2_foundation::NSPoint::new(0.0, 0.0));
        gradient.setEndPoint(objc2_foundation::NSPoint::new(0.9, 1.0));
        gradient.setOpacity(0.0);
        if let Some(layer) = root.layer() {
            layer.addSublayer(&gradient);
        }

        let lw = ((w - 2.0 * PAD - GAP) * 0.6).floor();
        let x0 = PAD;
        let y0 = top;

        // ── artwork ──
        let art_card = FlippedView::new(mtm, rect(x0, y0 + 2.0, ART, ART));
        make_card(&art_card, &p.card, 12.0);
        if let Some(l) = art_card.layer() {
            l.setMasksToBounds(true);
        }
        let art_placeholder = symbol_view(
            mtm,
            "music.note",
            34.0,
            &p.tertiary,
            rect(ART / 2.0 - 20.0, ART / 2.0 - 20.0, 40.0, 40.0),
            "No artwork",
        );
        let art = NSImageView::new(mtm);
        art.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        art.setFrame(rect(0.0, 0.0, ART, ART));
        art_card.addSubview(&art_placeholder);
        art_card.addSubview(&art);
        let st = state.clone();
        let t = targets.add(mtm, move |_| {
            // Bring the featured app to the front.
            match &st.borrow().featured {
                Featured::Player(pl) => glue::activate_app(pl.bundle_id()),
                Featured::App { bundle_id: Some(b), .. } => glue::activate_app(b),
                _ => {}
            }
        });
        art_card.addSubview(&click_overlay(mtm, &t, rect(0.0, 0.0, ART, ART), "Show player"));
        root.addSubview(&art_card);

        // ── title block ──
        let tx = x0 + ART + 14.0;
        let tw = lw - ART - 14.0;
        let badge_icon = symbol_view(mtm, "circle.fill", 9.0, &p.secondary, rect(tx, y0 + 5.0, 10.0, 10.0), "Source");
        let badge =
            label(mtm, "", &font(11.0, Weight::Semibold), &p.secondary, rect(tx + 15.0, y0 + 2.0, tw - 15.0, 15.0));
        let title =
            label(mtm, "Nothing playing", &font(17.0, Weight::Semibold), &p.primary, rect(tx, y0 + 20.0, tw, 22.0));
        let subtitle = label(mtm, "", &font(12.5, Weight::Regular), &p.secondary, rect(tx, y0 + 43.0, tw, 16.0));
        for v in [&*badge_icon as &NSView, &badge, &title, &subtitle] {
            root.addSubview(v);
        }

        // ── progress ──
        let st = state.clone();
        let t = targets.add(mtm, move |sender: Option<&AnyObject>| {
            let Some(s) = sender.and_then(|s| s.downcast_ref::<NSSlider>()) else { return };
            let mut st = st.borrow_mut();
            let Some(now) = st.now.as_mut() else { return };
            let Some(d) = now.duration else { return };
            let secs = s.doubleValue().clamp(0.0, 1.0) * d;
            now.position = Some(secs);
            now.sampled_at_ms = now_ms();
            glue::control(now.player, MediaAction::Seek(secs));
        });
        let progress = slider(mtm, &t, rect(tx - 2.0, y0 + 66.0, tw + 4.0, 16.0));
        progress.setContinuous(false);
        set_accessibility_label(&progress, "Playback position");
        let elapsed = label(mtm, "", &mono_digits(10.0, Weight::Medium), &p.tertiary, rect(tx, y0 + 83.0, 60.0, 13.0));
        let remaining = label(
            mtm,
            "",
            &mono_digits(10.0, Weight::Medium),
            &p.tertiary,
            rect(tx + tw - 60.0, y0 + 83.0, 60.0, 13.0),
        );
        remaining.setAlignment(NSTextAlignment::Right);
        for v in [&*progress as &NSView, &elapsed, &remaining] {
            root.addSubview(v);
        }

        // ── transport ──
        let cy = y0 + 100.0;
        let cx = tx + tw / 2.0;
        let mk = |sym: &str, size: f64, tip: &str, dx: f64, bw: f64, action: TransportFn| {
            let st = state.clone();
            let t = targets.add(mtm, move |_| action(&st));
            let b = symbol_button(mtm, sym, size, tip, &p.primary, &t, rect(cx + dx - bw / 2.0, cy, bw, 26.0));
            root.addSubview(&b);
            b
        };
        let shuffle = mk(
            "shuffle",
            12.0,
            "Shuffle",
            -96.0,
            24.0,
            Box::new(|st| {
                let st = st.borrow();
                if let Some(n) = &st.now {
                    glue::control(n.player, MediaAction::SetShuffle(!n.shuffle.unwrap_or(false)));
                }
            }),
        );
        let prev = mk(
            "backward.fill",
            16.0,
            "Previous track",
            -50.0,
            30.0,
            Box::new(|st| match &st.borrow().featured {
                Featured::Player(pl) => glue::control(*pl, MediaAction::Previous),
                Featured::App { .. } => {
                    glue::press(MediaKey::Previous);
                }
                Featured::Nothing => {}
            }),
        );
        let play = mk(
            "play.fill",
            22.0,
            "Play",
            0.0,
            38.0,
            Box::new(|st| {
                let mut st = st.borrow_mut();
                let featured = st.featured.clone();
                match featured {
                    Featured::Player(pl) => {
                        if let Some(n) = st.now.as_mut() {
                            // Optimistic: flip now, the provider confirms shortly.
                            let pos = n.position_at(now_ms());
                            n.position = pos;
                            n.sampled_at_ms = now_ms();
                            n.state = if n.is_playing() { PlaybackState::Paused } else { PlaybackState::Playing };
                        }
                        glue::control(pl, MediaAction::PlayPause);
                    }
                    Featured::App { .. } => {
                        glue::press(MediaKey::PlayPause);
                    }
                    Featured::Nothing => {}
                }
                drop(st);
                crate::app::with_controller(|c| c.media_tick());
            }),
        );
        let next = mk(
            "forward.fill",
            16.0,
            "Next track",
            50.0,
            30.0,
            Box::new(|st| match &st.borrow().featured {
                Featured::Player(pl) => glue::control(*pl, MediaAction::Next),
                Featured::App { .. } => {
                    glue::press(MediaKey::Next);
                }
                Featured::Nothing => {}
            }),
        );
        let repeat = mk(
            "repeat",
            12.0,
            "Repeat",
            96.0,
            24.0,
            Box::new(|st| {
                let st = st.borrow();
                if let Some(n) = &st.now {
                    glue::control(n.player, MediaAction::SetRepeat(!n.repeat.unwrap_or(false)));
                }
            }),
        );

        // Empty-state shortcuts.
        let t = targets.add(mtm, |_| glue::open_player(MediaPlayer::Spotify));
        let open_spotify = text_button(mtm, "Open Spotify", &t, rect(tx, y0 + 70.0, 108.0, 24.0));
        let t = targets.add(mtm, |_| glue::open_player(MediaPlayer::Music));
        let open_music = text_button(mtm, "Open Music", &t, rect(tx + 114.0, y0 + 70.0, 100.0, 24.0));
        root.addSubview(&open_spotify);
        root.addSubview(&open_music);

        // ── output card ──
        let oy = y0 + ART + 12.0;
        let out_card = FlippedView::new(mtm, rect(x0, oy, lw, 66.0));
        make_card(&out_card, &p.card, 14.0);
        let output_icon =
            symbol_view(mtm, "speaker.wave.2.fill", 16.0, &p.primary, rect(10.0, 9.0, 22.0, 20.0), "Output");
        let output_name =
            label(mtm, "Output", &font(12.5, Weight::Semibold), &p.primary, rect(40.0, 7.0, lw - 200.0, 16.0));
        let output_caps = label(mtm, "", &font(10.5, Weight::Regular), &p.tertiary, rect(40.0, 24.0, lw - 50.0, 14.0));
        let st = state.clone();
        let t = targets.add(mtm, move |sender: Option<&AnyObject>| {
            let Some(popup) = sender.and_then(|s| s.downcast_ref::<NSPopUpButton>()) else { return };
            let idx = popup.indexOfSelectedItem();
            if let Some(uid) = usize::try_from(idx).ok().and_then(|i| st.borrow().device_uids.get(i).cloned()) {
                glue::set_output(&uid);
            }
        });
        let output_popup = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(lw - 156.0, 5.0, 148.0, 22.0),
            false,
        );
        output_popup.setControlSize(NSControlSize::Small);
        output_popup.setFont(Some(&font(11.0, Weight::Regular)));
        set_accessibility_label(&output_popup, "Audio output device");
        // SAFETY: target kept alive by `targets`.
        unsafe {
            output_popup.setTarget(Some(t.as_any()));
            output_popup.setAction(Some(crate::ui::ActionTarget::action()));
        }
        out_card.addSubview(&symbol_view(
            mtm,
            "speaker.fill",
            10.0,
            &p.secondary,
            rect(12.0, 45.0, 14.0, 14.0),
            "Volume",
        ));
        let t = targets.add(mtm, |sender: Option<&AnyObject>| {
            if let Some(s) = sender.and_then(|s| s.downcast_ref::<NSSlider>()) {
                crate::controls::volume::set(s.floatValue());
            }
        });
        let volume = slider(mtm, &t, rect(32.0, 43.0, lw - 42.0, 16.0));
        volume.setContinuous(true);
        set_accessibility_label(&volume, "Output volume");
        for v in [&*output_icon as &NSView, &output_name, &output_caps, &output_popup, &volume] {
            out_card.addSubview(v);
        }
        root.addSubview(&out_card);

        // ── apps playing audio ──
        let sy = oy + 74.0;
        let sources_header =
            label(mtm, "Playing audio", &font(11.0, Weight::Semibold), &p.secondary, rect(x0 + 2.0, sy, lw, 14.0));
        let sources = FlippedView::new(mtm, rect(x0, sy + 18.0, lw, (h - sy - 18.0 - 10.0).max(20.0)));
        root.addSubview(&sources_header);
        root.addSubview(&sources);

        // ── playlists ──
        let rx = x0 + lw + GAP;
        let rw = w - PAD - rx;
        root.addSubview(&label(
            mtm,
            "Playlists",
            &font(12.0, Weight::Semibold),
            &p.secondary,
            rect(rx + 2.0, y0, 90.0, 16.0),
        ));
        let st = state.clone();
        let t = targets.add(mtm, move |sender: Option<&AnyObject>| {
            let Some(seg) = sender.and_then(|s| s.downcast_ref::<NSSegmentedControl>()) else { return };
            let i = seg.selectedSegment();
            {
                let mut st = st.borrow_mut();
                let chosen = usize::try_from(i).ok().and_then(|i| st.segment_players.get(i).copied());
                st.chosen_list_player = chosen;
                st.playlists_key = 0;
            }
            crate::app::with_controller(|c| c.media_rerender());
        });
        let labels = NSArray::from_retained_slice(&[NSString::from_str("Spotify"), NSString::from_str("Music")]);
        // SAFETY: target kept alive by `targets`.
        let list_switch = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &labels,
                NSSegmentSwitchTracking::SelectOne,
                Some(t.as_any()),
                Some(crate::ui::ActionTarget::action()),
                mtm,
            )
        };
        list_switch.setControlSize(NSControlSize::Mini);
        list_switch.setFrame(rect(rx + rw - 118.0, y0 - 2.0, 118.0, 18.0));
        root.addSubview(&list_switch);

        let lh = h - (y0 + 22.0) - 12.0;
        let list_card = FlippedView::new(mtm, rect(rx, y0 + 22.0, rw, lh));
        make_card(&list_card, &p.card, 14.0);
        let list_doc = FlippedView::new(mtm, rect(0.0, 0.0, rw, lh));
        let list_scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(0.0, 0.0, rw, lh));
        list_scroll.setDrawsBackground(false);
        list_scroll.setHasVerticalScroller(true);
        list_scroll.setAutohidesScrollers(true);
        list_scroll.setScrollerStyle(NSScrollerStyle::Overlay);
        list_scroll.setDocumentView(Some(&list_doc));
        list_card.addSubview(&list_scroll);
        let list_message = wrapping_label(
            mtm,
            "",
            &font(11.5, Weight::Regular),
            &p.secondary,
            rect(14.0, lh / 2.0 - 36.0, rw - 28.0, 44.0),
            3,
        );
        list_message.setAlignment(NSTextAlignment::Center);
        list_card.addSubview(&list_message);
        let list_action_kind = Rc::new(Cell::new(ListAction::None));
        let kind = list_action_kind.clone();
        let t = targets.add(mtm, move |_| match kind.get() {
            ListAction::None => {}
            ListAction::ConnectSpotify => glue::send(ProviderCommand::SpotifyConnect),
            ListAction::OpenSettings => crate::app::show_settings(),
            ListAction::AllowAutomation => glue::request_automation(),
            ListAction::OpenMusic => glue::open_player(MediaPlayer::Music),
            ListAction::Retry => glue::send(ProviderCommand::ReloadPlaylists),
        });
        let list_action = text_button(mtm, "", &t, rect(rw / 2.0 - 60.0, lh / 2.0 + 12.0, 120.0, 24.0));
        list_card.addSubview(&list_action);
        root.addSubview(&list_card);

        Self {
            root,
            mtm,
            p: p.clone(),
            _targets: targets,
            row_targets: Targets::default(),
            source_targets: Targets::default(),
            state,
            gradient,
            art,
            art_placeholder,
            badge_icon,
            badge,
            title,
            subtitle,
            progress,
            elapsed,
            remaining,
            shuffle,
            prev,
            play,
            next,
            repeat,
            open_spotify,
            open_music,
            output_icon,
            output_name,
            output_caps,
            output_popup,
            volume,
            sources_header,
            sources,
            sources_width: lw,
            list_switch,
            list_scroll,
            list_doc,
            list_width: rw,
            list_message,
            list_action,
            list_action_kind,
            ticker: RefCell::new(None),
        }
    }

    /// Start/stop the 1 Hz progress ticker (only while the page is on screen).
    pub fn set_active(&self, active: bool) {
        let mut t = self.ticker.borrow_mut();
        if active && t.is_none() {
            let block = block2::RcBlock::new(|_t: NonNull<objc2_foundation::NSTimer>| {
                crate::app::with_controller(|c| c.media_tick());
            });
            // SAFETY: documented NSTimer API.
            let timer =
                unsafe { objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_repeats_block(1.0, true, &block) };
            timer.setTolerance(0.2);
            *t = Some(timer);
            drop(t);
            self.tick();
        } else if !active {
            if let Some(timer) = t.take() {
                timer.invalidate();
            }
        }
    }

    /// Refresh the time-dependent bits (progress, clocks, play button).
    pub fn tick(&self) {
        let st = self.state.borrow();
        let Some(n) = st.now.as_ref() else { return };
        set_button_symbol(
            &self.play,
            if n.is_playing() { "pause.fill" } else { "play.fill" },
            22.0,
            if n.is_playing() { "Pause" } else { "Play" },
        );
        let pos = n.position_at(now_ms()).unwrap_or(0.0);
        match n.duration.filter(|d| *d > 0.0) {
            Some(d) => {
                if !(mouse_down() && self.progress.isHighlighted()) {
                    self.progress.setDoubleValue((pos / d).clamp(0.0, 1.0));
                }
                set_text(&self.elapsed, &clock(pos));
                set_text(&self.remaining, &format!("-{}", clock(d - pos)));
                self.progress.setEnabled(true);
            }
            None => {
                self.progress.setDoubleValue(0.0);
                self.progress.setEnabled(false);
                set_text(&self.elapsed, &clock(pos));
                set_text(&self.remaining, "");
            }
        }
    }

    pub fn update(&self, snap: &Snapshot) {
        let featured = featured(&snap.media, &snap.audio);
        {
            let mut st = self.state.borrow_mut();
            st.featured = featured.clone();
            st.now = match &featured {
                Featured::Player(pl) => snap.media.player(*pl).cloned(),
                _ => None,
            };
            st.media = snap.media.clone();
        }
        self.render_featured(snap, &featured);
        self.render_output(&snap.audio);
        self.render_sources(&snap.audio);
        self.render_playlists(&snap.media);
        self.tick();
    }

    fn set_tint(&self, color: Option<(f64, f64, f64)>) {
        match color {
            Some((r, g, b)) => {
                let top = rgba(r, g, b, 0.55).CGColor();
                let bottom = rgba(r, g, b, 0.0).CGColor();
                // SAFETY: CGColor is toll-free bridged to an Objective-C object.
                let objs: [&AnyObject; 2] = unsafe {
                    [
                        &*(&*top as *const objc2_core_graphics::CGColor as *const AnyObject),
                        &*(&*bottom as *const objc2_core_graphics::CGColor as *const AnyObject),
                    ]
                };
                let arr = NSArray::from_slice(&objs);
                // SAFETY: an array of CGColors, as CAGradientLayer expects.
                unsafe { self.gradient.setColors(Some(&arr)) };
                self.gradient.setOpacity(1.0);
            }
            None => self.gradient.setOpacity(0.0),
        }
    }

    fn render_featured(&self, snap: &Snapshot, featured: &Featured) {
        let p = &self.p;
        let (has_controls, has_progress) = match featured {
            Featured::Player(pl) => {
                let n = snap.media.player(*pl);
                set_symbol(&self.badge_icon, "circle.fill", 9.0, pl.display_name());
                self.badge_icon.setContentTintColor(Some(&rgb8(pl.color())));
                let state = match n.map(|n| n.state) {
                    Some(PlaybackState::Playing) => "Playing",
                    Some(PlaybackState::Paused) => "Paused",
                    _ => "Stopped",
                };
                set_text(&self.badge, &format!("{} · {state}", pl.display_name()));
                set_text(&self.title, n.map_or("", |n| n.title.as_str()));
                set_text(&self.subtitle, &n.map(NowPlaying::subtitle).unwrap_or_default());
                let art = n.and_then(|n| n.artwork_path.clone());
                let changed = self.state.borrow().art_path != art;
                if changed {
                    let img = art.as_deref().and_then(icons::file_image);
                    self.art.setImage(img.as_deref());
                    self.art_placeholder.setHidden(img.is_some());
                    let tint = img.as_deref().and_then(icons::average_color).or_else(|| {
                        let (r, g, b) = pl.color();
                        Some((r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0))
                    });
                    self.set_tint(tint);
                    self.state.borrow_mut().art_path = art;
                }
                if let Some(n) = n {
                    let on = |b: Option<bool>| if b == Some(true) { &p.accent } else { &p.secondary };
                    self.shuffle.setContentTintColor(Some(on(n.shuffle)));
                    self.repeat.setContentTintColor(Some(on(n.repeat)));
                    set_accessibility_label(&self.title, &format!("{}, {}", n.title, n.subtitle()));
                }
                (true, true)
            }
            Featured::App { name, bundle_id, app_path } => {
                set_symbol(&self.badge_icon, "waveform", 9.0, "Playing");
                self.badge_icon.setContentTintColor(Some(&p.success));
                set_text(&self.badge, "Playing audio");
                set_text(&self.title, name);
                let route = snap
                    .audio
                    .sources
                    .iter()
                    .find(|s| &s.name == name)
                    .map(|s| snap.audio.route_label(s))
                    .unwrap_or_default();
                set_text(&self.subtitle, &if route.is_empty() { String::new() } else { format!("Through {route}") });
                let icon = icons::source_icon(bundle_id.as_deref(), app_path.as_deref());
                self.art.setImage(icon.as_deref());
                self.art_placeholder.setHidden(icon.is_some());
                self.set_tint(None);
                self.state.borrow_mut().art_path = None;
                (true, false)
            }
            Featured::Nothing => {
                set_symbol(&self.badge_icon, "speaker.slash.fill", 9.0, "Silent");
                self.badge_icon.setContentTintColor(Some(&p.tertiary));
                set_text(&self.badge, "Silent");
                set_text(&self.title, "Nothing playing");
                set_text(&self.subtitle, "Start something in Spotify or Apple Music");
                self.art.setImage(None);
                self.art_placeholder.setHidden(false);
                self.set_tint(None);
                self.state.borrow_mut().art_path = None;
                (false, false)
            }
        };
        let rich = matches!(featured, Featured::Player(_));
        for b in [&self.prev, &self.play, &self.next] {
            b.setHidden(!has_controls);
        }
        self.shuffle.setHidden(!rich);
        self.repeat.setHidden(!rich);
        for v in [&*self.progress as &NSView, &self.elapsed, &self.remaining] {
            v.setHidden(!has_progress);
        }
        if !rich && has_controls {
            set_button_symbol(&self.play, "playpause.fill", 20.0, "Play or pause");
        }
        let empty = matches!(featured, Featured::Nothing);
        self.open_spotify.setHidden(!(empty && glue::is_installed(MediaPlayer::Spotify)));
        self.open_music.setHidden(!(empty && glue::is_installed(MediaPlayer::Music)));
    }

    fn render_output(&self, audio: &AudioState) {
        let Some(dev) = audio.default_device() else {
            set_text(&self.output_name, "No output device");
            set_text(&self.output_caps, "");
            return;
        };
        set_symbol(&self.output_icon, dev.symbol(), 16.0, &dev.name);
        set_text(&self.output_name, &dev.name);
        set_text(&self.output_caps, &dev.capabilities());
        set_accessibility_label(&self.output_name, &format!("Output: {}, {}", dev.name, dev.capabilities()));
        if let Some(v) = dev.volume {
            if !(mouse_down() && self.volume.isHighlighted()) {
                self.volume.setFloatValue(if dev.muted == Some(true) { 0.0 } else { v });
            }
            self.volume.setEnabled(true);
        } else {
            self.volume.setEnabled(false);
        }

        // Rebuild the popup only when the device list changes.
        let uids: Vec<String> = audio.devices.iter().map(|d| d.uid.clone()).collect();
        let changed = self.state.borrow().device_uids != uids;
        if changed {
            self.output_popup.removeAllItems();
            if let Some(menu) = self.output_popup.menu() {
                for d in &audio.devices {
                    // NSMenu keeps duplicate titles (NSPopUpButton's own API would merge them).
                    let item = objc2_app_kit::NSMenuItem::new(self.mtm);
                    item.setTitle(&NSString::from_str(&d.name));
                    item.setImage(crate::ui::symbol_image(d.symbol(), 12.0, &d.name).as_deref());
                    item.setToolTip(Some(&NSString::from_str(&d.capabilities())));
                    menu.addItem(&item);
                }
            }
            self.state.borrow_mut().device_uids = uids;
        }
        if let Some(i) = audio.devices.iter().position(|d| d.is_default) {
            self.output_popup.selectItemAtIndex(i as isize);
        }
    }

    fn render_sources(&self, audio: &AudioState) {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for s in &audio.sources {
            s.name.hash(&mut h);
            audio.route_label(s).hash(&mut h);
        }
        audio.sources_supported.hash(&mut h);
        let key = h.finish();
        if self.state.borrow().sources_key == key {
            return;
        }
        self.state.borrow_mut().sources_key = key;
        self.source_targets.clear();
        remove_all_subviews(&self.sources);
        let p = &self.p;
        if !audio.sources_supported {
            set_text(&self.sources_header, "Playing audio");
            self.sources.addSubview(&label(
                self.mtm,
                "Per-app audio needs macOS 14.2 or later",
                &font(11.0, Weight::Regular),
                &p.tertiary,
                rect(2.0, 0.0, self.sources_width, 15.0),
            ));
            return;
        }
        set_text(&self.sources_header, &format!("Playing audio ({})", audio.sources.len()));
        if audio.sources.is_empty() {
            self.sources.addSubview(&label(
                self.mtm,
                "No app is playing audio",
                &font(11.0, Weight::Regular),
                &p.tertiary,
                rect(2.0, 0.0, self.sources_width, 15.0),
            ));
            return;
        }
        // Two columns of compact "icon  App → Device" chips.
        let colw = (self.sources_width / 2.0).floor();
        let max_rows = ((self.sources.frame().size.height / 22.0).floor() as usize).max(1);
        for (i, s) in audio.sources.iter().take(max_rows * 2).enumerate() {
            let (col, row) = (i % 2, i / 2);
            let frame = rect(col as f64 * colw, row as f64 * 22.0, colw - 6.0, 20.0);
            let chip = FlippedView::new(self.mtm, frame);
            let icon = crate::views::app_icon_view(self.mtm, rect(2.0, 2.0, 16.0, 16.0));
            match icons::source_icon(s.bundle_id.as_deref(), s.app_path.as_deref()) {
                Some(img) => icon.setImage(Some(&img)),
                None => {
                    icon.setImage(crate::ui::symbol_image("app.fill", 12.0, &s.name).as_deref());
                    icon.setContentTintColor(Some(&p.secondary));
                }
            }
            let text = format!("{}  →  {}", s.name, audio.route_label(s));
            let l = label(self.mtm, &text, &font(11.0, Weight::Medium), &p.primary, rect(22.0, 3.0, colw - 30.0, 15.0));
            chip.addSubview(&icon);
            chip.addSubview(&l);
            if let Some(b) = s.bundle_id.clone() {
                let t = self.source_targets.add(self.mtm, move |_| glue::activate_app(&b));
                chip.addSubview(&click_overlay(
                    self.mtm,
                    &t,
                    rect(0.0, 0.0, colw - 6.0, 20.0),
                    &format!("Show {}", s.name),
                ));
            }
            set_accessibility_label(&chip, &text);
            self.sources.addSubview(&chip);
        }
    }

    /// Which player's playlists to list.
    fn list_player(&self, media: &MediaState) -> MediaPlayer {
        let st = self.state.borrow();
        if let Some(p) = st.chosen_list_player {
            return p;
        }
        if let Some(p) = media.primary() {
            return p.player;
        }
        MediaPlayer::ALL
            .into_iter()
            .find(|p| media.playlists_for(*p).next().is_some())
            .or_else(|| media.running.first().copied())
            .unwrap_or(if glue::is_installed(MediaPlayer::Spotify) { MediaPlayer::Spotify } else { MediaPlayer::Music })
    }

    fn render_playlists(&self, media: &MediaState) {
        let player = self.list_player(media);
        // Segment selection mirrors the listed player.
        self.state.borrow_mut().segment_players = MediaPlayer::ALL.to_vec();
        self.list_switch.setSelectedSegment(if player == MediaPlayer::Spotify { 0 } else { 1 });

        let status = media.playlist_status(player);
        let lists: Vec<&Playlist> = media.playlists_for(player).collect();
        let running = media.running.contains(&player);
        let denied = media.automation_denied.contains(&player);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        player.hash(&mut h);
        format!("{status:?}{running}{denied}").hash(&mut h);
        for l in &lists {
            l.id.hash(&mut h);
            l.name.hash(&mut h);
            l.track_count.hash(&mut h);
            l.artwork_path.hash(&mut h);
        }
        let key = h.finish();
        if self.state.borrow().playlists_key == key {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            st.playlists_key = key;
            st.list_player = Some(player);
        }
        self.row_targets.clear();
        remove_all_subviews(&self.list_doc);

        let (message, action, action_title): (Option<String>, ListAction, &str) = if denied
            || status == PlaylistStatus::NeedsAutomation
        {
            (
                Some(format!("Allow NotchForge to control {} to see playlists.", player.display_name())),
                ListAction::AllowAutomation,
                "Allow…",
            )
        } else if !lists.is_empty() {
            (None, ListAction::None, "")
        } else {
            match (player, &status) {
                (MediaPlayer::Spotify, PlaylistStatus::NeedsSpotifyLogin) => {
                    let has_id = crate::app::with_config(|c| !c.media.spotify_client_id.is_empty()).unwrap_or(false);
                    if has_id {
                        (
                            Some("Sign in to Spotify to list your playlists.".into()),
                            ListAction::ConnectSpotify,
                            "Connect Spotify",
                        )
                    } else {
                        (
                            Some("Add your Spotify client ID in Settings › Media to list playlists.".into()),
                            ListAction::OpenSettings,
                            "Open Settings",
                        )
                    }
                }
                (MediaPlayer::Music, _) if !running => {
                    (Some("Open Music to see your playlists.".into()), ListAction::OpenMusic, "Open Music")
                }
                (_, PlaylistStatus::Failed(e)) => {
                    (Some(format!("Couldn't load playlists: {e}")), ListAction::Retry, "Retry")
                }
                (_, PlaylistStatus::Loaded) => (Some("No playlists yet.".into()), ListAction::None, ""),
                _ => (Some("Loading playlists…".into()), ListAction::None, ""),
            }
        };
        self.list_action_kind.set(action);
        match message {
            Some(m) => {
                set_text(&self.list_message, &m);
                self.list_message.setHidden(false);
            }
            None => self.list_message.setHidden(true),
        }
        self.list_action.setTitle(&NSString::from_str(action_title));
        self.list_action.setHidden(action == ListAction::None);
        self.list_scroll.setHidden(lists.is_empty());

        let p = &self.p;
        let w = self.list_width;
        let visible_h = self.list_scroll.frame().size.height;
        let doc_h = (lists.len() as f64 * ROW_H + 8.0).max(visible_h);
        self.list_doc.setFrame(rect(0.0, 0.0, w, doc_h));
        for (i, pl) in lists.iter().enumerate() {
            let row = FlippedView::new(self.mtm, rect(0.0, 4.0 + i as f64 * ROW_H, w, ROW_H));
            let cover = FlippedView::new(self.mtm, rect(10.0, 6.0, 32.0, 32.0));
            make_card(&cover, &rgb8(player.color()), 6.0);
            if let Some(l) = cover.layer() {
                l.setMasksToBounds(true);
            }
            match pl.artwork_path.as_deref().and_then(icons::file_image) {
                Some(img) => {
                    let v = NSImageView::new(self.mtm);
                    v.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                    v.setFrame(rect(0.0, 0.0, 32.0, 32.0));
                    v.setImage(Some(&img));
                    cover.addSubview(&v);
                }
                None => cover.addSubview(&symbol_view(
                    self.mtm,
                    "music.note.list",
                    14.0,
                    &rgba(1.0, 1.0, 1.0, 0.9),
                    rect(7.0, 7.0, 18.0, 18.0),
                    &pl.name,
                )),
            }
            row.addSubview(&cover);
            row.addSubview(&label(
                self.mtm,
                &pl.name,
                &font(12.5, Weight::Medium),
                &p.primary,
                rect(50.0, 6.0, w - 84.0, 16.0),
            ));
            let mut detail = Vec::new();
            if let Some(c) = pl.track_count {
                detail.push(if c == 1 { "1 song".to_owned() } else { format!("{c} songs") });
            }
            if let Some(o) = &pl.owner {
                detail.push(o.clone());
            }
            row.addSubview(&label(
                self.mtm,
                &detail.join(" · "),
                &font(10.5, Weight::Regular),
                &p.tertiary,
                rect(50.0, 23.0, w - 84.0, 14.0),
            ));
            row.addSubview(&symbol_view(
                self.mtm,
                "play.fill",
                10.0,
                &p.secondary,
                rect(w - 28.0, 15.0, 14.0, 14.0),
                "Play",
            ));
            let (id, name) = (pl.id.clone(), pl.name.clone());
            let t = self.row_targets.add(self.mtm, move |_| {
                glue::control(player, MediaAction::PlayPlaylist(id.clone()));
            });
            row.addSubview(&click_overlay(self.mtm, &t, rect(0.0, 0.0, w, ROW_H), &format!("Play playlist {name}")));
            set_accessibility_label(&row, &format!("{}, {}", pl.name, detail.join(", ")));
            self.list_doc.addSubview(&row);
        }
        // Scroll back to the top when the list changes.
        self.list_doc.scrollPoint(objc2_foundation::NSPoint::new(0.0, 0.0));
    }

    /// (title shown, playlist rows) for diagnostics.
    pub fn debug_summary(&self) -> (String, usize) {
        (self.title.stringValue().to_string(), self.list_doc.subviews().len())
    }

    /// Force the next update to rebuild lists (e.g. the user switched player).
    pub fn invalidate(&self) {
        let mut st = self.state.borrow_mut();
        st.playlists_key = 0;
        st.sources_key = 0;
    }
}

impl Drop for MediaPage {
    fn drop(&mut self) {
        // The run loop retains scheduled timers; stop ours with the page.
        if let Some(t) = self.ticker.borrow_mut().take() {
            t.invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(75.4), "1:15");
        assert_eq!(clock(3700.0), "1:01:40");
    }
}
