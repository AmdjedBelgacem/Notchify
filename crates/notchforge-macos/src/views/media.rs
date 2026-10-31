//! "Now Playing" page of the expanded panel, built to the *Pinned · Now
//! Playing* board of the NotchForge design canvas (720 × 372 island).
//!
//! ```text
//! ┌──────────────────────────────────────────────┬──────────────────────────┐
//! │ ┌────────┐  ● Spotify · Playing on AirPods    │ Playlists  (Spotify|Music)│
//! │ │  art   │  Afterglow                         │ ▣ Night Drive      ▮▮▮   │
//! │ │  136   │  Neon Harbor — Night Transit       │ ▣ Deep Focus             │
//! │ └────────┘  ━━━━━━━━━━──────────  1:24  -2:17  │ ▣ Sunday Slow            │
//! │             ⇄    ⏮    ⏸    ⏭    ↻             │ ▣ Run Club               │
//! │ ┌ ◎ AirPods Pro                 (◉ Output) ┐  │ ▣ Rainy Paris            │
//! │ │   Bluetooth · 48 kHz · 24-bit · Stereo   │  │ ░░░░░ fade ░░░░░░░░░░░░ │
//! │ │ 🔈 ━━━━━━━━━━━━━━━━●────────────── 🔊    │  │                          │
//! │ └──────────────────────────────────────────┘  │                          │
//! │ Playing audio                                 │                          │
//! │ (▣ Spotify → AirPods Pro) (▣ Safari → AirPods)│                          │
//! └──────────────────────────────────────────────┴──────────────────────────┘
//! ```
//!
//! The page shows the *featured* source: the playing Spotify/Music player,
//! else another app producing audio (controlled with media keys), else an
//! empty state with shortcuts to open a player. The backdrop is a radial
//! wash of the cover's average colour.

use crate::icons;
use crate::media::{self as glue, MediaKey};
use crate::ui::{
    FlippedView, Palette, Targets, ThinSlider, Weight, attributed_parts, click_overlay, font, label, make_card,
    mono_digits, pill_button, rect, remove_all_subviews, rgb8, rgba, rich_label, set_accessibility_label,
    set_button_symbol, set_gradient, set_symbol, set_text, shape_view, style_pill, symbol_button, symbol_view,
    text_width, without_animation, wrapping_label,
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
    NSButton, NSImageScaling, NSImageView, NSMenu, NSMenuItem, NSScrollView, NSScrollerStyle, NSTextAlignment,
    NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSString};
use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;

const X0: f64 = 20.0;
const ART: f64 = 136.0;
const ROW_H: f64 = 48.0;
const EQ_DURATIONS: [f64; 3] = [0.42, 0.55, 0.38];

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
    devices: Vec<(String, String, String, bool)>,
    chosen_list_player: Option<MediaPlayer>,
    /// The playlist last started from the notch (highlighted while it plays).
    active_playlist: Option<String>,
    art_path: Option<PathBuf>,
    playlists_key: u64,
    sources_key: u64,
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

pub struct MediaPage {
    pub root: Retained<FlippedView>,
    mtm: MainThreadMarker,
    p: Palette,
    _targets: Targets,
    row_targets: Targets,
    source_targets: Targets,
    state: Shared,
    wash: Retained<objc2_quartz_core::CAGradientLayer>,

    art: Retained<NSImageView>,
    art_placeholder: Retained<NSImageView>,
    badge_dot: Retained<FlippedView>,
    badge: Retained<NSTextField>,
    title: Retained<NSTextField>,
    subtitle: Retained<NSTextField>,
    progress: Retained<ThinSlider>,
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
    output_button: Retained<NSButton>,
    volume: Retained<ThinSlider>,

    sources_header: Retained<NSTextField>,
    sources: Retained<FlippedView>,
    sources_width: f64,

    seg_spotify: Retained<NSButton>,
    seg_music: Retained<NSButton>,
    list_scroll: Retained<NSScrollView>,
    list_doc: Retained<FlippedView>,
    list_width: f64,
    list_fade: Retained<objc2_quartz_core::CAGradientLayer>,
    list_message: Retained<NSTextField>,
    list_action: Retained<NSButton>,
    list_action_kind: Rc<Cell<ListAction>>,

    ticker: RefCell<Option<Retained<objc2_foundation::NSTimer>>>,
}

fn seg_style(b: &NSButton, title: &str, on: bool) {
    let fg = if on { rgba(1.0, 1.0, 1.0, 1.0) } else { rgba(1.0, 1.0, 1.0, 0.55) };
    let bg = if on { rgba(1.0, 1.0, 1.0, 0.18) } else { rgba(1.0, 1.0, 1.0, 0.0) };
    style_pill(b, title, None, &font(11.0, Weight::Semibold), &fg, &bg, 6.0);
}

/// Three bars animated by Core Animation (render server: no CPU here).
pub fn equalizer_view(
    mtm: MainThreadMarker,
    frame: objc2_foundation::NSRect,
    color: &objc2_app_kit::NSColor,
) -> Retained<NSView> {
    use objc2_quartz_core::{CABasicAnimation, CALayer, CAMediaTiming};
    let v = NSView::initWithFrame(NSView::alloc(mtm), frame);
    v.setWantsLayer(true);
    if let Some(root) = v.layer() {
        for (i, d) in EQ_DURATIONS.iter().enumerate() {
            let bar = CALayer::new();
            bar.setBounds(rect(0.0, 0.0, 2.5, frame.size.height));
            bar.setAnchorPoint(NSPoint::new(0.5, 0.0));
            bar.setPosition(NSPoint::new(i as f64 * 4.5 + 1.25, 0.0));
            bar.setCornerRadius(1.0);
            bar.setBackgroundColor(Some(&color.CGColor()));
            let anim = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("transform.scale.y")));
            let (from, to) = (objc2_foundation::NSNumber::new_f64(0.3), objc2_foundation::NSNumber::new_f64(1.0));
            // SAFETY: NSNumber values for a scalar key path.
            unsafe {
                anim.setFromValue(Some(&from));
                anim.setToValue(Some(&to));
            }
            anim.setDuration(*d);
            anim.setTimeOffset(i as f64 * 0.13);
            anim.setAutoreverses(true);
            anim.setRepeatCount(f32::INFINITY);
            if !crate::views::system_reduce_motion() {
                bar.addAnimation_forKey(&anim, Some(&NSString::from_str("eq")));
            }
            root.addSublayer(&bar);
        }
    }
    set_accessibility_label(&v, "Now playing");
    v
}

impl MediaPage {
    /// `top` is the first content row below the notch band (notch height + 8).
    pub fn new(mtm: MainThreadMarker, w: f64, h: f64, top: f64, p: &Palette) -> Self {
        let nh = top - 8.0;
        let root = FlippedView::new(mtm, rect(0.0, 0.0, w, h));
        let targets = Targets::default();
        let state: Shared = Rc::default();
        let white = |a: f64| rgba(1.0, 1.0, 1.0, a);

        // ── cover-colour wash (radial, top-left) ──
        root.setWantsLayer(true);
        let wash = objc2_quartz_core::CAGradientLayer::layer();
        // SAFETY: "radial" is kCAGradientLayerRadial.
        wash.setType(&NSString::from_str("radial"));
        wash.setFrame(rect(0.0, 0.0, w, h));
        wash.setStartPoint(NSPoint::new(0.08, 0.0));
        wash.setEndPoint(NSPoint::new(0.55, 0.85));
        wash.setOpacity(0.0);
        if let Some(layer) = root.layer() {
            layer.addSublayer(&wash);
        }

        let lw = ((w - 56.0) * 0.612).floor();
        let rx = X0 + lw + 16.0;
        let rw = w - 20.0 - rx;

        // ── artwork ──
        let art_shadow = FlippedView::new(mtm, rect(X0, nh + 18.0, ART, ART));
        art_shadow.setWantsLayer(true);
        if let Some(l) = art_shadow.layer() {
            l.setShadowOpacity(0.5);
            l.setShadowRadius(14.0);
            l.setShadowOffset(objc2_foundation::NSSize::new(0.0, -8.0));
        }
        let art_card = shape_view(mtm, rect(0.0, 0.0, ART, ART), &white(0.07), 14.0);
        if let Some(l) = art_card.layer() {
            l.setMasksToBounds(true);
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&white(0.12).CGColor()));
        }
        let art_placeholder = symbol_view(
            mtm,
            "music.note",
            36.0,
            &p.tertiary,
            rect(ART / 2.0 - 22.0, ART / 2.0 - 22.0, 44.0, 44.0),
            "No artwork",
        );
        let art = NSImageView::new(mtm);
        art.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        art.setFrame(rect(0.0, 0.0, ART, ART));
        art_card.addSubview(&art_placeholder);
        art_card.addSubview(&art);
        let st = state.clone();
        let t = targets.add(mtm, move |_| match &st.borrow().featured {
            Featured::Player(pl) => glue::activate_app(pl.bundle_id()),
            Featured::App { bundle_id: Some(b), .. } => glue::activate_app(b),
            _ => {}
        });
        art_card.addSubview(&click_overlay(mtm, &t, rect(0.0, 0.0, ART, ART), "Show player"));
        art_shadow.addSubview(&art_card);
        root.addSubview(&art_shadow);

        // ── title block ──
        let tx = X0 + ART + 18.0;
        let tw = lw - ART - 18.0;
        let badge_dot = shape_view(mtm, rect(tx, nh + 24.0, 7.0, 7.0), &p.success, 3.5);
        let badge =
            label(mtm, "", &font(11.5, Weight::Semibold), &p.secondary, rect(tx + 13.0, nh + 20.0, tw - 13.0, 15.0));
        let title =
            label(mtm, "Nothing playing", &font(20.0, Weight::Semibold), &p.primary, rect(tx, nh + 40.0, tw, 25.0));
        let subtitle = label(mtm, "", &font(13.0, Weight::Regular), &p.secondary, rect(tx, nh + 66.0, tw, 17.0));
        for v in [&*badge_dot as &NSView, &badge, &title, &subtitle] {
            root.addSubview(v);
        }

        // ── progress ──
        let progress = ThinSlider::new(mtm, rect(tx, nh + 94.0, tw, 16.0), false, &white(0.92));
        set_accessibility_label(&progress, "Playback position");
        let st = state.clone();
        progress.set_on_commit(move |v| {
            let mut st = st.borrow_mut();
            let Some(now) = st.now.as_mut() else { return };
            let Some(d) = now.duration else { return };
            let secs = v * d;
            now.position = Some(secs);
            now.sampled_at_ms = now_ms();
            glue::control(now.player, MediaAction::Seek(secs));
        });
        let elapsed = label(mtm, "", &mono_digits(11.0, Weight::Medium), &p.tertiary, rect(tx, nh + 110.0, 70.0, 14.0));
        let remaining = label(
            mtm,
            "",
            &mono_digits(11.0, Weight::Medium),
            &p.tertiary,
            rect(tx + tw - 70.0, nh + 110.0, 70.0, 14.0),
        );
        remaining.setAlignment(NSTextAlignment::Right);
        for v in [&*progress as &NSView, &elapsed, &remaining] {
            root.addSubview(v);
        }

        // ── transport: space-between across the text column ──
        let widths = [32.0, 40.0, 48.0, 40.0, 32.0];
        let gap = ((tw - 4.0) - widths.iter().sum::<f64>()) / 4.0;
        let mut xs = [0.0; 5];
        let mut x = tx + 2.0;
        for (i, bw) in widths.iter().enumerate() {
            xs[i] = x;
            x += bw + gap;
        }
        let cy = nh + 153.0;
        let mk = |i: usize, sym: &str, size: f64, tip: &str, action: TransportFn| {
            let st = state.clone();
            let t = targets.add(mtm, move |_| action(&st));
            let bw = widths[i];
            let b = symbol_button(mtm, sym, size, tip, &p.primary, &t, rect(xs[i], cy - bw / 2.0, bw, bw));
            root.addSubview(&b);
            b
        };
        let shuffle = mk(
            0,
            "shuffle",
            14.0,
            "Shuffle",
            Box::new(|st| {
                if let Some(n) = &st.borrow().now {
                    glue::control(n.player, MediaAction::SetShuffle(!n.shuffle.unwrap_or(false)));
                }
            }),
        );
        let prev = mk(
            1,
            "backward.fill",
            19.0,
            "Previous track",
            Box::new(|st| match &st.borrow().featured {
                Featured::Player(pl) => glue::control(*pl, MediaAction::Previous),
                Featured::App { .. } => {
                    glue::press(MediaKey::Previous);
                }
                Featured::Nothing => {}
            }),
        );
        let play = mk(
            2,
            "play.fill",
            27.0,
            "Play",
            Box::new(|st| {
                let mut st = st.borrow_mut();
                match st.featured.clone() {
                    Featured::Player(pl) => {
                        if let Some(n) = st.now.as_mut() {
                            // Optimistic: flip now, the provider confirms shortly.
                            n.position = n.position_at(now_ms());
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
            3,
            "forward.fill",
            19.0,
            "Next track",
            Box::new(|st| match &st.borrow().featured {
                Featured::Player(pl) => glue::control(*pl, MediaAction::Next),
                Featured::App { .. } => {
                    glue::press(MediaKey::Next);
                }
                Featured::Nothing => {}
            }),
        );
        let repeat = mk(
            4,
            "repeat",
            14.0,
            "Repeat",
            Box::new(|st| {
                if let Some(n) = &st.borrow().now {
                    glue::control(n.player, MediaAction::SetRepeat(!n.repeat.unwrap_or(false)));
                }
            }),
        );

        // Empty-state shortcuts (where the progress bar sits when playing).
        let t = targets.add(mtm, |_| glue::open_player(MediaPlayer::Spotify));
        let open_spotify = pill_button(
            mtm,
            "Open Spotify",
            None,
            &font(12.0, Weight::Semibold),
            &p.primary,
            &white(0.12),
            13.0,
            &t,
            rect(tx, nh + 96.0, 104.0, 26.0),
        );
        let t = targets.add(mtm, |_| glue::open_player(MediaPlayer::Music));
        let open_music = pill_button(
            mtm,
            "Open Music",
            None,
            &font(12.0, Weight::Semibold),
            &p.primary,
            &white(0.12),
            13.0,
            &t,
            rect(tx + 112.0, nh + 96.0, 96.0, 26.0),
        );
        root.addSubview(&open_spotify);
        root.addSubview(&open_music);

        // ── output card ──
        let oy = nh + 172.0;
        let out_card = shape_view(mtm, rect(X0, oy, lw, 78.0), &white(0.07), 16.0);
        if let Some(l) = out_card.layer() {
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&white(0.08).CGColor()));
        }
        let icon_disc = shape_view(mtm, rect(14.0, 12.0, 30.0, 30.0), &white(0.12), 15.0);
        let output_icon = symbol_view(mtm, "headphones", 14.0, &p.primary, rect(7.0, 7.0, 16.0, 16.0), "Output");
        icon_disc.addSubview(&output_icon);
        let output_name =
            label(mtm, "Output", &font(13.5, Weight::Semibold), &p.primary, rect(55.0, 11.0, lw - 170.0, 17.0));
        let output_caps = label(mtm, "", &font(11.0, Weight::Regular), &p.tertiary, rect(55.0, 28.0, lw - 170.0, 14.0));
        let st = state.clone();
        let t = targets.add(mtm, move |sender: Option<&AnyObject>| {
            let Some(b) = sender.and_then(|s| s.downcast_ref::<NSButton>()) else { return };
            output_menu(b, &st.borrow().devices);
        });
        let output_button = pill_button(
            mtm,
            "Output",
            Some("airplayaudio"),
            &font(12.0, Weight::Semibold),
            &p.primary,
            &white(0.12),
            13.0,
            &t,
            rect(lw - 14.0 - 88.0, 14.0, 88.0, 26.0),
        );
        out_card.addSubview(&symbol_view(
            mtm,
            "speaker.fill",
            10.0,
            &p.secondary,
            rect(14.0, 52.0, 13.0, 13.0),
            "Volume",
        ));
        let volume = ThinSlider::new(mtm, rect(34.0, 48.0, lw - 74.0, 20.0), true, &p.primary);
        set_accessibility_label(&volume, "Output volume");
        volume.set_on_change(|v| {
            crate::controls::volume::set(v as f32);
        });
        out_card.addSubview(&symbol_view(
            mtm,
            "speaker.wave.2.fill",
            11.0,
            &p.secondary,
            rect(lw - 32.0, 51.0, 18.0, 14.0),
            "Louder",
        ));
        for v in [&*icon_disc as &NSView, &output_name, &output_caps, &output_button, &volume] {
            out_card.addSubview(v);
        }
        root.addSubview(&out_card);

        // ── apps playing audio ──
        let sy = nh + 266.0;
        let sources_header =
            label(mtm, "Playing audio", &font(11.5, Weight::Semibold), &p.secondary, rect(X0 + 2.0, sy, lw, 15.0));
        let sources = FlippedView::new(mtm, rect(X0, sy + 20.0, lw, (h - sy - 20.0 - 12.0).max(30.0)));
        root.addSubview(&sources_header);
        root.addSubview(&sources);

        // ── playlists card ──
        let cy0 = nh + 12.0;
        let ch = h - cy0 - 18.0;
        let card = shape_view(mtm, rect(rx, cy0, rw, ch), &white(0.06), 18.0);
        if let Some(l) = card.layer() {
            l.setMasksToBounds(true);
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&white(0.08).CGColor()));
        }
        card.addSubview(&label(
            mtm,
            "Playlists",
            &font(13.0, Weight::Semibold),
            &p.primary,
            rect(14.0, 13.0, 100.0, 17.0),
        ));
        let seg_track = shape_view(mtm, rect(rw - 12.0 - 112.0, 10.0, 112.0, 24.0), &white(0.08), 8.0);
        let pick = |player: MediaPlayer| {
            let st = state.clone();
            move |_: Option<&AnyObject>| {
                {
                    let mut st = st.borrow_mut();
                    st.chosen_list_player = Some(player);
                    st.playlists_key = 0;
                }
                crate::app::with_controller(|c| c.media_rerender());
            }
        };
        let t = targets.add(mtm, pick(MediaPlayer::Spotify));
        let seg_spotify = pill_button(
            mtm,
            "Spotify",
            None,
            &font(11.0, Weight::Semibold),
            &p.primary,
            &white(0.18),
            6.0,
            &t,
            rect(2.0, 2.0, 58.0, 20.0),
        );
        let t = targets.add(mtm, pick(MediaPlayer::Music));
        let seg_music = pill_button(
            mtm,
            "Music",
            None,
            &font(11.0, Weight::Semibold),
            &p.secondary,
            &white(0.0),
            6.0,
            &t,
            rect(60.0, 2.0, 50.0, 20.0),
        );
        seg_track.addSubview(&seg_spotify);
        seg_track.addSubview(&seg_music);
        card.addSubview(&seg_track);

        let lh = ch - 40.0;
        let list_doc = FlippedView::new(mtm, rect(0.0, 0.0, rw - 12.0, lh));
        let list_scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(6.0, 40.0, rw - 12.0, lh));
        list_scroll.setDrawsBackground(false);
        list_scroll.setHasVerticalScroller(true);
        list_scroll.setAutohidesScrollers(true);
        list_scroll.setScrollerStyle(NSScrollerStyle::Overlay);
        list_scroll.setDocumentView(Some(&list_doc));
        card.addSubview(&list_scroll);

        // Bottom fade so the list reads as scrollable.
        let fade_host = FlippedView::new(mtm, rect(0.0, ch - 40.0, rw, 40.0));
        fade_host.setWantsLayer(true);
        let list_fade = objc2_quartz_core::CAGradientLayer::layer();
        list_fade.setFrame(rect(0.0, 0.0, rw, 40.0));
        set_gradient(&list_fade, &[(0.06, 0.06, 0.06, 0.0), (0.06, 0.06, 0.06, 0.95)]);
        list_fade.setStartPoint(NSPoint::new(0.5, 0.0));
        list_fade.setEndPoint(NSPoint::new(0.5, 1.0));
        if let Some(l) = fade_host.layer() {
            l.addSublayer(&list_fade);
        }
        card.addSubview(&fade_host);

        let list_message = wrapping_label(
            mtm,
            "",
            &font(12.0, Weight::Regular),
            &p.secondary,
            rect(18.0, ch / 2.0 - 34.0, rw - 36.0, 48.0),
            3,
        );
        list_message.setAlignment(NSTextAlignment::Center);
        card.addSubview(&list_message);
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
        let list_action = pill_button(
            mtm,
            "",
            None,
            &font(12.0, Weight::Semibold),
            &p.primary,
            &p.accent,
            13.0,
            &t,
            rect(rw / 2.0 - 70.0, ch / 2.0 + 20.0, 140.0, 26.0),
        );
        card.addSubview(&list_action);
        root.addSubview(&card);

        Self {
            root,
            mtm,
            p: p.clone(),
            _targets: targets,
            row_targets: Targets::default(),
            source_targets: Targets::default(),
            state,
            wash,
            art,
            art_placeholder,
            badge_dot,
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
            output_button,
            volume,
            sources_header,
            sources,
            sources_width: lw,
            seg_spotify,
            seg_music,
            list_scroll,
            list_doc,
            list_width: rw - 12.0,
            list_fade,
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
            27.0,
            if n.is_playing() { "Pause" } else { "Play" },
        );
        let pos = n.position_at(now_ms()).unwrap_or(0.0);
        match n.duration.filter(|d| *d > 0.0) {
            Some(d) => {
                self.progress.set_value(pos / d);
                set_text(&self.elapsed, &clock(pos));
                set_text(&self.remaining, &format!("-{}", clock(d - pos)));
            }
            None => {
                self.progress.set_value(0.0);
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
        }
        self.render_featured(snap, &featured);
        self.render_output(&snap.audio);
        self.render_sources(&snap.audio);
        self.render_playlists(&snap.media);
        self.tick();
    }

    fn set_wash(&self, color: Option<(f64, f64, f64)>) {
        match color {
            Some((r, g, b)) => {
                set_gradient(&self.wash, &[(r, g, b, 0.34), (r, g, b, 0.0)]);
                self.wash.setOpacity(1.0);
            }
            None => self.wash.setOpacity(0.0),
        }
    }

    fn render_featured(&self, snap: &Snapshot, featured: &Featured) {
        let p = &self.p;
        let f = font(11.5, Weight::Semibold);
        let fm = font(11.5, Weight::Medium);
        let (has_controls, has_progress) = match featured {
            Featured::Player(pl) => {
                let n = snap.media.player(*pl);
                let brand = rgb8(pl.color());
                make_card(&self.badge_dot, &brand, 3.5);
                let route = snap.audio.default_device().map(|d| d.name.clone()).unwrap_or_default();
                let state = match n.map(|n| n.state) {
                    Some(PlaybackState::Playing) if !route.is_empty() => format!("  ·  Playing on {route}"),
                    Some(PlaybackState::Playing) => "  ·  Playing".into(),
                    Some(PlaybackState::Paused) => "  ·  Paused".into(),
                    _ => String::new(),
                };
                let short = match pl {
                    MediaPlayer::Spotify => "Spotify",
                    MediaPlayer::Music => "Music",
                };
                self.badge
                    .setAttributedStringValue(&attributed_parts(&[(short, &f, &brand), (&state, &fm, &p.tertiary)]));
                set_text(&self.title, n.map_or("", |n| n.title.as_str()));
                set_text(&self.subtitle, &n.map(NowPlaying::subtitle).unwrap_or_default());
                let art = n.and_then(|n| n.artwork_path.clone());
                if self.state.borrow().art_path != art || art.is_none() {
                    let img = art.as_deref().and_then(icons::file_image);
                    self.art.setImage(img.as_deref());
                    self.art_placeholder.setHidden(img.is_some());
                    let (r, g, b) = pl.color();
                    let tint = img.as_deref().and_then(icons::average_color).unwrap_or((
                        r as f64 / 255.0,
                        g as f64 / 255.0,
                        b as f64 / 255.0,
                    ));
                    self.set_wash(Some(tint));
                    self.state.borrow_mut().art_path = art;
                }
                if let Some(n) = n {
                    let on = |b: Option<bool>| if b == Some(true) { brand.clone() } else { p.secondary.clone() };
                    self.shuffle.setContentTintColor(Some(&on(n.shuffle)));
                    self.repeat.setContentTintColor(Some(&on(n.repeat)));
                    self.progress.set_fill_color(&rgba(1.0, 1.0, 1.0, 0.92));
                    set_accessibility_label(&self.title, &format!("{}, {}", n.title, n.subtitle()));
                }
                (true, true)
            }
            Featured::App { name, bundle_id, app_path } => {
                make_card(&self.badge_dot, &p.success, 3.5);
                self.badge.setAttributedStringValue(&attributed_parts(&[("Playing audio", &f, &p.success)]));
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
                self.set_wash(None);
                self.state.borrow_mut().art_path = None;
                (true, false)
            }
            Featured::Nothing => {
                make_card(&self.badge_dot, &p.tertiary, 3.5);
                self.badge.setAttributedStringValue(&attributed_parts(&[("Silent", &f, &p.tertiary)]));
                set_text(&self.title, "Nothing playing");
                set_text(&self.subtitle, "Start something in Spotify or Apple Music");
                self.art.setImage(None);
                self.art_placeholder.setHidden(false);
                self.set_wash(None);
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
            set_button_symbol(&self.play, "playpause.fill", 24.0, "Play or pause");
        }
        let empty = matches!(featured, Featured::Nothing);
        self.open_spotify.setHidden(!(empty && glue::is_installed(MediaPlayer::Spotify)));
        self.open_music.setHidden(!(empty && glue::is_installed(MediaPlayer::Music)));
    }

    fn render_output(&self, audio: &AudioState) {
        self.state.borrow_mut().devices =
            audio.devices.iter().map(|d| (d.uid.clone(), d.name.clone(), d.capabilities(), d.is_default)).collect();
        let Some(dev) = audio.default_device() else {
            set_text(&self.output_name, "No output device");
            set_text(&self.output_caps, "");
            return;
        };
        set_symbol(&self.output_icon, dev.symbol(), 14.0, &dev.name);
        set_text(&self.output_name, &dev.name);
        let caps = dev.capabilities();
        let avail = self.output_caps.frame().size.width - 4.0;
        let fits = crate::ui::text_width(&crate::ui::attributed(&caps, &font(11.0, Weight::Regular), &self.p.tertiary))
            <= avail;
        // Drop the "up to …" tail before truncating mid-word.
        let caps = if fits { caps } else { caps.split(" · up to").next().unwrap_or(&caps).to_owned() };
        set_text(&self.output_caps, &caps);
        set_accessibility_label(&self.output_name, &format!("Output: {}, {}", dev.name, dev.capabilities()));
        self.output_button.setEnabled(audio.devices.len() > 1);
        match dev.volume {
            Some(v) => {
                self.volume.set_value(if dev.muted == Some(true) { 0.0 } else { v as f64 });
                self.volume.setHidden(false);
            }
            None => self.volume.setHidden(true),
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
        let note = |text: &str| {
            self.sources.addSubview(&label(
                self.mtm,
                text,
                &font(11.5, Weight::Regular),
                &p.tertiary,
                rect(2.0, 6.0, self.sources_width, 16.0),
            ));
        };
        if !audio.sources_supported {
            set_text(&self.sources_header, "Playing audio");
            note("Per-app audio needs macOS 14.2 or later");
            return;
        }
        set_text(&self.sources_header, "Playing audio");
        if audio.sources.is_empty() {
            note("No app is playing audio");
            return;
        }
        // Chips flow left to right, wrapping onto a second row.
        let fm = font(12.0, Weight::Medium);
        let (mut x, mut y) = (0.0, 0.0);
        let max_y = self.sources.frame().size.height - 30.0;
        for s in &audio.sources {
            let route = audio.route_label(s);
            let text = attributed_parts(&[
                (&s.name, &fm, &p.primary),
                ("  →  ", &fm, &p.tertiary),
                (&route, &fm, &p.secondary),
            ]);
            let cw = (31.0 + text_width(&text) + 12.0).min(self.sources_width);
            if x > 0.0 && x + cw > self.sources_width {
                x = 0.0;
                y += 36.0;
            }
            if y > max_y {
                break;
            }
            let chip = shape_view(self.mtm, rect(x, y, cw, 30.0), &rgba(1.0, 1.0, 1.0, 0.07), 15.0);
            let icon = crate::views::app_icon_view(self.mtm, rect(7.0, 6.0, 18.0, 18.0));
            match icons::source_icon(s.bundle_id.as_deref(), s.app_path.as_deref()) {
                Some(img) => icon.setImage(Some(&img)),
                None => {
                    icon.setImage(crate::ui::symbol_image("app.fill", 12.0, &s.name).as_deref());
                    icon.setContentTintColor(Some(&p.secondary));
                }
            }
            chip.addSubview(&icon);
            chip.addSubview(&rich_label(self.mtm, &text, rect(31.0, 7.0, cw - 38.0, 16.0)));
            if let Some(b) = s.bundle_id.clone() {
                let t = self.source_targets.add(self.mtm, move |_| glue::activate_app(&b));
                chip.addSubview(&click_overlay(self.mtm, &t, rect(0.0, 0.0, cw, 30.0), &format!("Show {}", s.name)));
            }
            set_accessibility_label(&chip, &format!("{} is playing through {route}", s.name));
            self.sources.addSubview(&chip);
            x += cw + 8.0;
        }
    }

    /// Which player's playlists to list.
    fn list_player(&self, media: &MediaState) -> MediaPlayer {
        if let Some(p) = self.state.borrow().chosen_list_player {
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
        seg_style(&self.seg_spotify, "Spotify", player == MediaPlayer::Spotify);
        seg_style(&self.seg_music, "Music", player == MediaPlayer::Music);

        let status = media.playlist_status(player);
        let lists: Vec<&Playlist> = media.playlists_for(player).collect();
        let running = media.running.contains(&player);
        let denied = media.automation_denied.contains(&player);
        let playing = media.player(player).is_some_and(NowPlaying::is_playing);
        let active = self.state.borrow().active_playlist.clone().filter(|_| playing);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        player.hash(&mut h);
        format!("{status:?}{running}{denied}{active:?}").hash(&mut h);
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
        self.state.borrow_mut().playlists_key = key;
        self.row_targets.clear();
        remove_all_subviews(&self.list_doc);

        let (message, action, action_title): (Option<String>, ListAction, &str) = if denied
            || status == PlaylistStatus::NeedsAutomation
        {
            (
                Some(format!("Allow NotchForge to control {} to see your playlists.", player.display_name())),
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
                            Some("Add your Spotify client ID in Settings › Media to list your playlists.".into()),
                            ListAction::OpenSettings,
                            "Open Settings",
                        )
                    }
                }
                (MediaPlayer::Music, _) if !running => {
                    (Some("Open Music to see your playlists.".into()), ListAction::OpenMusic, "Open Music")
                }
                (_, PlaylistStatus::Failed(e)) => {
                    (Some(format!("Couldn't load playlists. {e}")), ListAction::Retry, "Retry")
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
        style_pill(
            &self.list_action,
            action_title,
            None,
            &font(12.0, Weight::Semibold),
            &self.p.primary,
            &self.p.accent,
            13.0,
        );
        self.list_action.setHidden(action == ListAction::None);
        self.list_scroll.setHidden(lists.is_empty());
        without_animation(|| self.list_fade.setHidden(lists.len() < 6));

        let p = &self.p;
        let w = self.list_width;
        let brand = rgb8(player.color());
        let visible_h = self.list_scroll.frame().size.height;
        let doc_h = (lists.len() as f64 * ROW_H + 30.0).max(visible_h);
        self.list_doc.setFrame(rect(0.0, 0.0, w, doc_h));
        for (i, pl) in lists.iter().enumerate() {
            let is_active = active.as_deref() == Some(pl.id.as_str());
            let row_bg = if is_active { rgba(1.0, 1.0, 1.0, 0.09) } else { rgba(1.0, 1.0, 1.0, 0.0) };
            let row = shape_view(self.mtm, rect(0.0, i as f64 * ROW_H, w, ROW_H), &row_bg, 11.0);
            let cover = shape_view(self.mtm, rect(8.0, 6.0, 36.0, 36.0), &brand, 8.0);
            if let Some(l) = cover.layer() {
                l.setMasksToBounds(true);
            }
            match pl.artwork_path.as_deref().and_then(icons::file_image) {
                Some(img) => {
                    let v = NSImageView::new(self.mtm);
                    v.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                    v.setFrame(rect(0.0, 0.0, 36.0, 36.0));
                    v.setImage(Some(&img));
                    cover.addSubview(&v);
                }
                None => cover.addSubview(&symbol_view(
                    self.mtm,
                    "music.note.list",
                    15.0,
                    &rgba(1.0, 1.0, 1.0, 0.92),
                    rect(8.0, 8.0, 20.0, 20.0),
                    &pl.name,
                )),
            }
            row.addSubview(&cover);
            row.addSubview(&label(
                self.mtm,
                &pl.name,
                &font(13.0, Weight::Medium),
                &p.primary,
                rect(54.0, 7.0, w - 84.0, 17.0),
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
                &font(11.0, Weight::Regular),
                &p.tertiary,
                rect(54.0, 25.0, w - 84.0, 14.0),
            ));
            if is_active {
                row.addSubview(&equalizer_view(self.mtm, rect(w - 26.0, 18.0, 12.0, 12.0), &brand));
            }
            let (id, name) = (pl.id.clone(), pl.name.clone());
            let st = self.state.clone();
            let t = self.row_targets.add(self.mtm, move |_| {
                st.borrow_mut().active_playlist = Some(id.clone());
                glue::control(player, MediaAction::PlayPlaylist(id.clone()));
            });
            row.addSubview(&click_overlay(self.mtm, &t, rect(0.0, 0.0, w, ROW_H), &format!("Play playlist {name}")));
            set_accessibility_label(&row, &format!("{}, {}", pl.name, detail.join(", ")));
            self.list_doc.addSubview(&row);
        }
        self.list_doc.scrollPoint(NSPoint::new(0.0, 0.0));
    }

    /// (title shown, playlist rows) for diagnostics.
    pub fn debug_summary(&self) -> (String, usize) {
        (self.title.stringValue().to_string(), self.list_doc.subviews().len())
    }

    /// Force the next update to rebuild lists (e.g. the user switched player).
    /// Mark a playlist as the one playing (self-test renders of the board).
    pub fn set_active_playlist(&self, id: Option<String>) {
        self.state.borrow_mut().active_playlist = id;
    }

    pub fn invalidate(&self) {
        let mut st = self.state.borrow_mut();
        st.playlists_key = 0;
        st.sources_key = 0;
    }
}

/// The "Output" pill's menu: every output, with its capabilities.
fn output_menu(anchor: &NSButton, devices: &[(String, String, String, bool)]) {
    let mtm = MainThreadMarker::from(anchor);
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    // Targets must outlive the menu's tracking; the menu is modal, so a local
    // store dropped after `popUp…` returns is enough.
    let targets = Targets::default();
    for (uid, name, caps, is_default) in devices {
        let uid = uid.clone();
        let t = targets.add(mtm, move |_| glue::set_output(&uid));
        let item = NSMenuItem::new(mtm);
        let f = font(13.0, Weight::Regular);
        let fs = font(11.0, Weight::Regular);
        let title = attributed_parts(&[
            (name, &f, &objc2_app_kit::NSColor::labelColor()),
            (&format!("\n{caps}"), &fs, &objc2_app_kit::NSColor::secondaryLabelColor()),
        ]);
        item.setAttributedTitle(Some(&title));
        item.setTitle(&NSString::from_str(name));
        item.setState(if *is_default {
            objc2_app_kit::NSControlStateValueOn
        } else {
            objc2_app_kit::NSControlStateValueOff
        });
        // SAFETY: target kept alive until the menu closes (below).
        unsafe {
            item.setTarget(Some(t.as_any()));
            item.setAction(Some(crate::ui::ActionTarget::action()));
        }
        menu.addItem(&item);
    }
    let size = anchor.frame().size;
    menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(0.0, size.height + 4.0), Some(anchor));
    drop(targets);
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
