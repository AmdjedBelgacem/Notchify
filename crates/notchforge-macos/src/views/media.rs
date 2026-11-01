//! "Now Playing" page of the expanded panel, built to the *Pinned · Now
//! Playing, glass* board of the NotchForge design canvas.
//!
//! ```text
//! ╭───────────────────────────────────────────╮ ╭──────────────────────────╮
//! │ ┌────────┐ Spotify · Playing               │ │ Playlists (Spotify|Music)│
//! │ │  art   │ Afterglow                       │ │ ▣ Night Drive       ▮▮▮  │
//! │ │        │ Neon Harbor                     │ │ ▣ Deep Focus             │
//! │ │        │ Night Transit                   │ │ ▣ Sunday Slow            │
//! │ └────────┘ ▓▓▓▓▓▓▓░░░░░░░░░░  1:24  -2:17  │ │ ▣ Run Club               │
//! │    ⇄      ⏮      (⏸)      ⏭      ↻        │ │ ▣ Rainy Paris            │
//! │ (🔈▓▓▓▓▓▓▓▓▓░░░░░░░░░) (🎧 AirPods Pro)    │ │                          │
//! │ ▣ Safari is also playing     48 kHz · 24-bit│ │                          │
//! ╰───────────────────────────────────────────╯ ╰──────────────────────────╯
//! ```
//!
//! The page shows the *featured* source: the playing Spotify/Music player,
//! else another app producing audio (controlled with media keys), else an
//! empty state with shortcuts to open a player. The player tile is tinted from
//! the cover's average colour.

use crate::icons;
use crate::media::{self as glue, MediaKey};
use crate::ui::{
    FlippedView, Palette, Targets, ThinSlider, Weight, attributed_parts, click_overlay, font, label, make_card,
    mono_digits, pill_button, rect, remove_all_subviews, rgb8, rgba, set_accessibility_label, set_button_symbol,
    set_gradient, set_text, shape_view, style_pill, symbol_button, symbol_view, text_width, without_animation,
    wrapping_label,
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

const PAD: f64 = 14.0;
/// Tallest a playlist row grows to when there are only a few.
const MAX_ROW_H: f64 = 64.0;
const VISIBLE_ROWS: f64 = 5.0;
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
    AllowAutomation,
    OpenMusic,
    Retry,
    /// Give up the Spotify sign-in waiting on the browser.
    CancelSpotify,
}

pub struct MediaPage {
    pub root: Retained<FlippedView>,
    mtm: MainThreadMarker,
    p: Palette,
    _targets: Targets,
    row_targets: Targets,
    source_targets: Targets,
    state: Shared,
    tint: Retained<objc2_quartz_core::CAGradientLayer>,

    art: Retained<NSImageView>,
    art_placeholder: Retained<NSImageView>,
    badge: Retained<NSTextField>,
    title: Retained<NSTextField>,
    subtitle: Retained<NSTextField>,
    album: Retained<NSTextField>,
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

    output_button: Retained<NSButton>,
    output_right: f64,
    volume: Retained<ThinSlider>,
    volume_y: f64,

    caps: Retained<NSTextField>,
    sources: Retained<FlippedView>,
    sources_width: f64,

    seg_spotify: Retained<NSButton>,
    seg_music: Retained<NSButton>,
    list_scroll: Retained<NSScrollView>,
    list_doc: Retained<FlippedView>,
    list_width: f64,
    list_message: Retained<NSTextField>,
    list_action: Retained<NSButton>,
    list_action_kind: Rc<Cell<ListAction>>,

    ticker: RefCell<Option<Retained<objc2_foundation::NSTimer>>>,
}

fn seg_style(b: &NSButton, title: &str, on: bool) {
    let fg = if on { rgba(1.0, 1.0, 1.0, 1.0) } else { rgba(1.0, 1.0, 1.0, 0.72) };
    let bg = if on { rgba(1.0, 1.0, 1.0, 0.28) } else { rgba(1.0, 1.0, 1.0, 0.0) };
    style_pill(b, title, None, &font(12.0, Weight::Semibold), &fg, &bg, 12.0);
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

        let top_y = nh + 6.0;
        let ph = h - top_y - PAD;
        let lw = ((w - 2.0 * PAD - 10.0) * 420.0 / 682.0).floor();
        let rx = PAD + lw + 10.0;
        let rw = w - PAD - rx;

        // ── player tile, tinted from the cover ──
        let player = super::expanded::glass_tile(mtm, rect(PAD, top_y, lw, ph));
        let tint = objc2_quartz_core::CAGradientLayer::layer();
        tint.setFrame(rect(0.0, 0.0, lw, ph));
        tint.setStartPoint(NSPoint::new(0.0, 0.0));
        tint.setEndPoint(NSPoint::new(1.0, 1.0));
        tint.setOpacity(0.0);
        if let Some(l) = player.layer() {
            l.setCornerRadius(24.0);
            l.setMasksToBounds(true);
            l.addSublayer(&tint);
        }
        root.addSubview(&player);

        // The artwork takes what the rows below it leave.
        let art_side = (ph - 162.0).clamp(96.0, 150.0);

        // ── artwork ──
        let art_shadow = FlippedView::new(mtm, rect(PAD, PAD, art_side, art_side));
        art_shadow.setWantsLayer(true);
        if let Some(l) = art_shadow.layer() {
            l.setShadowOpacity(0.45);
            l.setShadowRadius(14.0);
            l.setShadowOffset(objc2_foundation::NSSize::new(0.0, -8.0));
        }
        let art_card = shape_view(mtm, rect(0.0, 0.0, art_side, art_side), &white(0.1), 14.0);
        if let Some(l) = art_card.layer() {
            l.setMasksToBounds(true);
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&white(0.16).CGColor()));
        }
        let art_placeholder = symbol_view(
            mtm,
            "music.note",
            36.0,
            &p.secondary,
            rect(art_side / 2.0 - 22.0, art_side / 2.0 - 22.0, 44.0, 44.0),
            "No artwork",
        );
        let art = NSImageView::new(mtm);
        art.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        art.setFrame(rect(0.0, 0.0, art_side, art_side));
        art_card.addSubview(&art_placeholder);
        art_card.addSubview(&art);
        let st = state.clone();
        let t = targets.add(mtm, move |_| match &st.borrow().featured {
            Featured::Player(pl) => glue::activate_app(pl.bundle_id()),
            Featured::App { bundle_id: Some(b), .. } => glue::activate_app(b),
            _ => {}
        });
        art_card.addSubview(&click_overlay(mtm, &t, rect(0.0, 0.0, art_side, art_side), "Show player"));
        art_shadow.addSubview(&art_card);
        player.addSubview(&art_shadow);

        // ── title block ──
        let tx = PAD + art_side + PAD;
        let tw = lw - tx - PAD;
        let badge = label(mtm, "", &font(12.0, Weight::Semibold), &white(0.84), rect(tx, PAD + 2.0, tw, 16.0));
        let title =
            label(mtm, "Nothing playing", &font(26.0, Weight::Bold), &p.primary, rect(tx, PAD + 20.0, tw, 32.0));
        let subtitle = label(mtm, "", &font(15.0, Weight::Medium), &white(0.88), rect(tx, PAD + 52.0, tw, 20.0));
        let album = label(mtm, "", &font(13.0, Weight::Regular), &white(0.76), rect(tx, PAD + 72.0, tw, 17.0));
        for v in [&*badge as &NSView, &title, &subtitle, &album] {
            player.addSubview(v);
        }

        // ── progress, level with the bottom of the artwork ──
        let bar_y = PAD + art_side - 14.0 - 19.0;
        let progress =
            ThinSlider::capsule(mtm, rect(tx, bar_y, tw, 16.0), 7.0, &white(0.24), &rgba(1.0, 1.0, 1.0, 1.0));
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
        let times_y = PAD + art_side - 14.0;
        let elapsed = label(mtm, "", &mono_digits(11.0, Weight::Medium), &white(0.82), rect(tx, times_y, 70.0, 14.0));
        let remaining =
            label(mtm, "", &mono_digits(11.0, Weight::Medium), &white(0.82), rect(tx + tw - 70.0, times_y, 70.0, 14.0));
        remaining.setAlignment(NSTextAlignment::Right);
        for v in [&*progress as &NSView, &elapsed, &remaining] {
            player.addSubview(v);
        }

        // ── transport: space-between across the tile ──
        let row_y = PAD + art_side + 8.0;
        let widths = [40.0, 52.0, 52.0, 52.0, 40.0];
        let span = lw - 2.0 * 20.0;
        let gap = (span - widths.iter().sum::<f64>()) / 4.0;
        let mut xs = [0.0; 5];
        let mut x = 20.0;
        for (i, bw) in widths.iter().enumerate() {
            xs[i] = x;
            x += bw + gap;
        }
        let mk = |i: usize, sym: &str, size: f64, tip: &str, action: TransportFn| {
            let st = state.clone();
            let t = targets.add(mtm, move |_| action(&st));
            let bw = widths[i];
            let bh = if i == 2 {
                52.0
            } else if i == 0 || i == 4 {
                40.0
            } else {
                44.0
            };
            let b = symbol_button(mtm, sym, size, tip, &p.primary, &t, rect(xs[i], row_y + (52.0 - bh) / 2.0, bw, bh));
            player.addSubview(&b);
            b
        };
        let shuffle = mk(
            0,
            "shuffle",
            16.0,
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
            22.0,
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
            24.0,
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
        make_card(&play, &white(0.2), 26.0);
        if let Some(l) = play.layer() {
            l.setBorderWidth(0.5);
            l.setBorderColor(Some(&white(0.34).CGColor()));
        }
        let next = mk(
            3,
            "forward.fill",
            22.0,
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
            16.0,
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
            &white(0.16),
            14.0,
            &t,
            rect(tx, bar_y + 2.0, 104.0, 28.0),
        );
        let t = targets.add(mtm, |_| glue::open_player(MediaPlayer::Music));
        let open_music = pill_button(
            mtm,
            "Open Music",
            None,
            &font(12.0, Weight::Semibold),
            &p.primary,
            &white(0.16),
            14.0,
            &t,
            rect(tx + 112.0, bar_y + 2.0, 96.0, 28.0),
        );
        player.addSubview(&open_spotify);
        player.addSubview(&open_music);

        // ── volume and output ──
        let volume_y = row_y + 52.0 + 8.0;
        let output_right = lw - PAD;
        let volume =
            ThinSlider::capsule(mtm, rect(PAD, volume_y, lw - 2.0 * PAD, 36.0), 36.0, &white(0.16), &white(0.94));
        volume.addSubview(&symbol_view(
            mtm,
            "speaker.wave.1.fill",
            14.0,
            &rgba(0.11, 0.11, 0.118, 1.0),
            rect(11.0, 10.0, 16.0, 16.0),
            "Volume",
        ));
        set_accessibility_label(&volume, "Output volume");
        volume.set_on_change(|v| {
            crate::controls::volume::set(v as f32);
        });
        player.addSubview(&volume);
        let st = state.clone();
        let t = targets.add(mtm, move |sender: Option<&AnyObject>| {
            let Some(b) = sender.and_then(|s| s.downcast_ref::<NSButton>()) else { return };
            output_menu(b, &st.borrow().devices);
        });
        let output_button = pill_button(
            mtm,
            "Output",
            Some("airplayaudio"),
            &font(12.5, Weight::Semibold),
            &p.primary,
            &white(0.16),
            18.0,
            &t,
            rect(output_right - 90.0, volume_y, 90.0, 36.0),
        );
        player.addSubview(&output_button);

        // ── other apps making sound, and the output's format ──
        let by = ph - 12.0 - 24.0;
        let caps_w = 120.0;
        let caps = label(
            mtm,
            "",
            &mono_digits(12.0, Weight::Regular),
            &white(0.84),
            rect(lw - PAD - caps_w, by + 4.0, caps_w, 16.0),
        );
        caps.setAlignment(NSTextAlignment::Right);
        player.addSubview(&caps);
        let sources_width = lw - 2.0 * PAD - caps_w - 6.0;
        let sources = FlippedView::new(mtm, rect(PAD, by, sources_width, 24.0));
        player.addSubview(&sources);

        // ── playlists tile ──
        let card = super::expanded::glass_tile(mtm, rect(rx, top_y, rw, ph));
        if let Some(l) = card.layer() {
            l.setCornerRadius(24.0);
            l.setMasksToBounds(true);
        }
        card.addSubview(&label(
            mtm,
            "Playlists",
            &font(15.0, Weight::Semibold),
            &p.primary,
            rect(14.0, 15.0, rw - 140.0, 19.0),
        ));
        let seg_track = shape_view(mtm, rect(rw - 10.0 - 122.0, 10.0, 122.0, 28.0), &white(0.12), 14.0);
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
            &font(12.0, Weight::Semibold),
            &p.primary,
            &white(0.28),
            12.0,
            &t,
            rect(2.0, 2.0, 64.0, 24.0),
        );
        let t = targets.add(mtm, pick(MediaPlayer::Music));
        let seg_music = pill_button(
            mtm,
            "Music",
            None,
            &font(12.0, Weight::Semibold),
            &p.secondary,
            &white(0.0),
            12.0,
            &t,
            rect(66.0, 2.0, 54.0, 24.0),
        );
        seg_track.addSubview(&seg_spotify);
        seg_track.addSubview(&seg_music);
        card.addSubview(&seg_track);

        let lh = ph - 48.0 - 6.0;
        let list_doc = FlippedView::new(mtm, rect(0.0, 0.0, rw - 12.0, lh));
        let list_scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(6.0, 48.0, rw - 12.0, lh));
        list_scroll.setDrawsBackground(false);
        list_scroll.setHasVerticalScroller(true);
        list_scroll.setAutohidesScrollers(true);
        list_scroll.setScrollerStyle(NSScrollerStyle::Overlay);
        list_scroll.setDocumentView(Some(&list_doc));
        card.addSubview(&list_scroll);

        let list_message = wrapping_label(
            mtm,
            "",
            &font(12.0, Weight::Regular),
            &p.secondary,
            rect(18.0, ph / 2.0 - 34.0, rw - 36.0, 48.0),
            3,
        );
        list_message.setAlignment(NSTextAlignment::Center);
        card.addSubview(&list_message);
        let list_action_kind = Rc::new(Cell::new(ListAction::None));
        let kind = list_action_kind.clone();
        let t = targets.add(mtm, move |_| match kind.get() {
            ListAction::None => {}
            ListAction::ConnectSpotify => glue::send(ProviderCommand::SpotifyConnect),
            ListAction::AllowAutomation => glue::request_automation(),
            ListAction::OpenMusic => glue::open_player(MediaPlayer::Music),
            ListAction::Retry => glue::send(ProviderCommand::ReloadPlaylists),
            ListAction::CancelSpotify => glue::send(ProviderCommand::SpotifyCancel),
        });
        let list_action = pill_button(
            mtm,
            "",
            None,
            &font(12.0, Weight::Semibold),
            &p.primary,
            &p.accent,
            14.0,
            &t,
            rect(rw / 2.0 - 70.0, ph / 2.0 + 20.0, 140.0, 28.0),
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
            tint,
            art,
            art_placeholder,
            badge,
            title,
            subtitle,
            album,
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
            output_button,
            output_right,
            volume,
            volume_y,
            caps,
            sources,
            sources_width,
            seg_spotify,
            seg_music,
            list_scroll,
            list_doc,
            list_width: rw - 12.0,
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
            24.0,
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
        self.render_sources(&snap.audio, &featured);
        self.render_playlists(&snap.media);
        self.tick();
    }

    /// Tint the player tile from the cover, dark enough to keep white text legible.
    fn set_wash(&self, color: Option<(f64, f64, f64)>) {
        without_animation(|| match color {
            Some((r, g, b)) => {
                let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                let k = if lum > 0.34 { 0.34 / lum } else { 1.0 };
                let (r, g, b) = (r * k, g * k, b * k);
                set_gradient(
                    &self.tint,
                    &[(r, g, b, 0.84), (r * 0.62, g * 0.62, b * 0.62, 0.8), (r * 0.3, g * 0.3, b * 0.3, 0.78)],
                );
                self.tint.setOpacity(1.0);
            }
            None => self.tint.setOpacity(0.0),
        });
    }

    fn render_featured(&self, snap: &Snapshot, featured: &Featured) {
        let p = &self.p;
        let (has_controls, has_progress) = match featured {
            Featured::Player(pl) => {
                let n = snap.media.player(*pl);
                let state = match n.map(|n| n.state) {
                    Some(PlaybackState::Playing) => " · Playing",
                    Some(PlaybackState::Paused) => " · Paused",
                    _ => "",
                };
                let short = match (pl, n) {
                    (MediaPlayer::Spotify, _) => "Spotify",
                    (MediaPlayer::Music, _) => "Music",
                    (MediaPlayer::Browser, Some(n)) => n.source_name(),
                    (MediaPlayer::Browser, None) => "Browser",
                };
                set_text(&self.badge, &format!("{short}{state}"));
                set_text(&self.title, n.map_or("", |n| n.title.as_str()));
                let artist = n.map(|n| if n.artist.is_empty() { n.subtitle() } else { n.artist.clone() });
                set_text(&self.subtitle, &artist.unwrap_or_default());
                set_text(&self.album, n.map_or("", |n| if n.artist.is_empty() { "" } else { n.album.as_str() }));
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
                    let on = |b: Option<bool>| rgba(1.0, 1.0, 1.0, if b == Some(true) { 0.28 } else { 0.0 });
                    make_card(&self.shuffle, &on(n.shuffle), 20.0);
                    make_card(&self.repeat, &on(n.repeat), 20.0);
                    self.shuffle.setContentTintColor(Some(&p.primary));
                    self.repeat.setContentTintColor(Some(&p.primary));
                    set_accessibility_label(&self.title, &format!("{}, {}", n.title, n.subtitle()));
                }
                (true, n.is_none_or(|n| n.duration.is_some()))
            }
            Featured::App { name, bundle_id, app_path } => {
                set_text(&self.badge, "Playing audio");
                set_text(&self.album, "");
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
                set_text(&self.badge, "Silent");
                set_text(&self.album, "");
                set_text(&self.title, "Nothing playing");
                set_text(&self.subtitle, "Start something in Spotify or Apple Music");
                self.art.setImage(None);
                self.art_placeholder.setHidden(false);
                self.set_wash(None);
                self.state.borrow_mut().art_path = None;
                (false, false)
            }
        };
        let rich = matches!(featured, Featured::Player(p) if *p != MediaPlayer::Browser);
        for b in [&self.prev, &self.play, &self.next] {
            b.setHidden(!has_controls);
        }
        self.shuffle.setHidden(!rich);
        self.repeat.setHidden(!rich);
        for v in [&*self.progress as &NSView, &self.elapsed, &self.remaining] {
            v.setHidden(!has_progress);
        }
        if !rich && has_controls {
            set_button_symbol(&self.play, "playpause.fill", 22.0, "Play or pause");
        }
        let empty = matches!(featured, Featured::Nothing);
        self.open_spotify.setHidden(!(empty && glue::is_installed(MediaPlayer::Spotify)));
        self.open_music.setHidden(!(empty && glue::is_installed(MediaPlayer::Music)));
    }

    fn render_output(&self, audio: &AudioState) {
        self.state.borrow_mut().devices =
            audio.devices.iter().map(|d| (d.uid.clone(), d.name.clone(), d.capabilities(), d.is_default)).collect();
        let f = font(12.5, Weight::Semibold);
        let name = audio.default_device().map_or("No output", |d| d.name.as_str());
        let symbol = audio.default_device().map_or("airplayaudio", |d| d.symbol());
        let bw = (text_width(&crate::ui::attributed(name, &f, &self.p.primary)) + 50.0).min(self.output_right * 0.5);
        style_pill(&self.output_button, name, Some(symbol), &f, &self.p.primary, &rgba(1.0, 1.0, 1.0, 0.16), 18.0);
        self.output_button.setFrame(rect(self.output_right - bw, self.volume_y, bw, 36.0));
        self.volume.set_slider_frame(rect(PAD, self.volume_y, self.output_right - bw - 8.0 - PAD, 36.0));
        self.output_button.setEnabled(!audio.devices.is_empty());
        let Some(dev) = audio.default_device() else {
            set_text(&self.caps, "");
            self.volume.setHidden(true);
            return;
        };
        // The format only: the output's name is on the button beside the volume.
        let format: Vec<String> = dev
            .capabilities()
            .split(" · ")
            .filter(|part| !part.starts_with("up to") && (part.contains("kHz") || part.contains("bit")))
            .map(str::to_owned)
            .collect();
        set_text(&self.caps, &format.join(" · "));
        set_accessibility_label(&self.output_button, &format!("Output: {}, {}", dev.name, dev.capabilities()));
        match dev.volume {
            Some(v) => {
                self.volume.set_value(if dev.muted == Some(true) { 0.0 } else { v as f64 });
                self.volume.setHidden(false);
            }
            None => self.volume.setHidden(true),
        }
    }

    /// One line for the other apps making sound; the featured one is the tile.
    fn render_sources(&self, audio: &AudioState, featured: &Featured) {
        let is_featured = |s: &notchforge_core::model::AudioSource| match featured {
            Featured::Player(pl) => s.bundle_id.as_deref() == Some(pl.bundle_id()),
            Featured::App { name, .. } => &s.name == name,
            Featured::Nothing => false,
        };
        let others: Vec<_> = audio.sources.iter().filter(|s| !is_featured(s)).collect();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for s in &others {
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
        let color = rgba(1.0, 1.0, 1.0, 0.84);
        let f = font(12.0, Weight::Regular);
        let width = self.sources_width;
        let text_at = |x: f64, text: &str| {
            self.sources.addSubview(&label(self.mtm, text, &f, &color, rect(x, 4.0, width - x, 16.0)));
        };
        if !audio.sources_supported {
            text_at(0.0, "Per-app audio needs macOS 14.2 or later");
            return;
        }
        let Some(s) = others.first() else {
            text_at(0.0, "No other app is playing audio");
            return;
        };
        let slot = rect(0.0, 3.0, 18.0, 18.0);
        let boxed = icons::source_icon(s.bundle_id.as_deref(), s.app_path.as_deref())
            .and_then(|img| crate::ui::image_box(self.mtm, slot, &img));
        if let Some(b) = boxed {
            self.sources.addSubview(&b);
        } else if s.bundle_id.as_deref() == Some("com.spotify.client") {
            // The board's Spotify tile: green, with the black mark.
            let tile = shape_view(self.mtm, slot, &crate::ui::rgb8(MediaPlayer::Spotify.color()), 5.0);
            tile.addSubview(&crate::ui::spotify_mark(self.mtm, rect(3.0, 3.0, 12.0, 12.0)));
            self.sources.addSubview(&tile);
        } else {
            let icon = crate::views::app_icon_view(self.mtm, slot);
            icon.setImage(crate::ui::symbol_image("app.fill", 12.0, &s.name).as_deref());
            icon.setContentTintColor(Some(&color));
            self.sources.addSubview(&icon);
        }
        let route = audio.route_label(s);
        let text = if others.len() > 1 {
            format!("{} and {} more also playing", s.name, others.len() - 1)
        } else {
            format!("{} is also playing on {route}", s.name)
        };
        text_at(25.0, &text);
        if let Some(b) = s.bundle_id.clone() {
            let t = self.source_targets.add(self.mtm, move |_| glue::activate_app(&b));
            self.sources.addSubview(&click_overlay(
                self.mtm,
                &t,
                rect(0.0, 0.0, width, 24.0),
                &format!("Show {}", s.name),
            ));
        }
        set_accessibility_label(&self.sources, &text);
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

        let (message, action, action_title): (Option<String>, ListAction, &str) =
            if denied || status == PlaylistStatus::NeedsAutomation {
                (
                    Some(format!("Allow Notchisland to control {} to see your playlists.", player.display_name())),
                    ListAction::AllowAutomation,
                    "Allow…",
                )
            } else if !lists.is_empty() {
                (None, ListAction::None, "")
            } else {
                match (player, &status) {
                    (MediaPlayer::Spotify, PlaylistStatus::SigningIn) => (
                        Some("Waiting for you to approve Notchisland in your browser…".into()),
                        ListAction::CancelSpotify,
                        "Cancel",
                    ),
                    (MediaPlayer::Spotify, PlaylistStatus::NeedsSpotifyLogin) => (
                        Some("Sign in to Spotify to list your playlists.".into()),
                        ListAction::ConnectSpotify,
                        "Connect Spotify",
                    ),
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
            14.0,
        );
        self.list_action.setHidden(action == ListAction::None);
        self.list_scroll.setHidden(lists.is_empty());

        let p = &self.p;
        let w = self.list_width;
        let brand = rgb8(player.color());
        let visible_h = self.list_scroll.frame().size.height;
        // Five rows fill the tile; a shorter list spreads out, a longer one scrolls.
        let row_h = if lists.is_empty() || lists.len() as f64 >= VISIBLE_ROWS {
            visible_h / VISIBLE_ROWS
        } else {
            (visible_h / lists.len() as f64).min(MAX_ROW_H)
        };
        let doc_h = (lists.len() as f64 * row_h).max(visible_h);
        self.list_doc.setFrame(rect(0.0, 0.0, w, doc_h));
        let detail_color = rgba(1.0, 1.0, 1.0, 0.74);
        for (i, pl) in lists.iter().enumerate() {
            let is_active = active.as_deref() == Some(pl.id.as_str());
            let row_bg = if is_active { rgba(1.0, 1.0, 1.0, 0.14) } else { rgba(1.0, 1.0, 1.0, 0.0) };
            let row = shape_view(self.mtm, rect(0.0, i as f64 * row_h, w, row_h), &row_bg, 13.0);
            let cy = (row_h - 40.0) / 2.0;
            let cover = shape_view(self.mtm, rect(8.0, cy, 40.0, 40.0), &brand, 8.0);
            if let Some(l) = cover.layer() {
                l.setMasksToBounds(true);
            }
            match pl.artwork_path.as_deref().and_then(icons::file_image) {
                Some(img) => {
                    let v = NSImageView::new(self.mtm);
                    v.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                    v.setFrame(rect(0.0, 0.0, 40.0, 40.0));
                    v.setImage(Some(&img));
                    cover.addSubview(&v);
                }
                None => cover.addSubview(&symbol_view(
                    self.mtm,
                    "music.note.list",
                    16.0,
                    &rgba(1.0, 1.0, 1.0, 0.92),
                    rect(10.0, 10.0, 20.0, 20.0),
                    &pl.name,
                )),
            }
            row.addSubview(&cover);
            let ty = (row_h - 33.0) / 2.0;
            row.addSubview(&label(
                self.mtm,
                &pl.name,
                &font(13.5, Weight::Medium),
                &p.primary,
                rect(58.0, ty, w - 88.0, 18.0),
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
                &font(12.0, Weight::Regular),
                &detail_color,
                rect(58.0, ty + 18.0, w - 88.0, 15.0),
            ));
            if is_active {
                row.addSubview(&equalizer_view(self.mtm, rect(w - 26.0, (row_h - 12.0) / 2.0, 12.0, 12.0), &brand));
            }
            let (id, name) = (pl.id.clone(), pl.name.clone());
            let st = self.state.clone();
            let t = self.row_targets.add(self.mtm, move |_| {
                st.borrow_mut().active_playlist = Some(id.clone());
                glue::control(player, MediaAction::PlayPlaylist(id.clone()));
            });
            row.addSubview(&click_overlay(self.mtm, &t, rect(0.0, 0.0, w, row_h), &format!("Play playlist {name}")));
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
