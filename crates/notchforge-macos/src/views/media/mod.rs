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

mod library;

use crate::icons;
use crate::media::{self as glue, MediaKey};
use crate::ui::{
    FlippedView, Palette, Targets, ThinSlider, Weight, attributed, attributed_parts, click_overlay, font, label,
    make_card, mono_digits, pill_button, rect, remove_all_subviews, rgb8, rgba, set_accessibility_label,
    set_button_symbol, set_gradient, set_text, shape_view, style_pill, symbol_button, symbol_view, text_width,
    without_animation, wrapping_label,
};
use notchforge_core::engine::Snapshot;
use notchforge_core::model::{
    AudioState, MediaAction, MediaPlayer, MediaState, NowPlaying, PlaybackState, Playlist, PlaylistStatus, UpNextVideo,
};
use notchforge_core::provider::ProviderCommand;
use notchforge_core::time::now_ms;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSButton, NSImageScaling, NSImageView, NSMenu, NSMenuItem, NSScrollView, NSScrollerStyle, NSSlider,
    NSTextAlignment, NSTextField, NSView,
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
    inputs: Vec<(String, String, bool)>,
    chosen_list_player: Option<MediaPlayer>,
    /// The player the user picked in the sources bar (or via the segments):
    /// the card and the controls follow it until they pick another one or it
    /// stops running.
    pinned: Option<MediaPlayer>,
    /// The playlist last started from the notch (highlighted while it plays).
    active_playlist: Option<String>,
    art_path: Option<PathBuf>,
    playlists_key: u64,
    sources_key: u64,
    /// The Lyrics view is showing (instead of playlists / up next).
    show_lyrics: bool,
    /// The search field is open; results replace the list.
    searching: bool,
    search_results: Vec<UpNextVideo>,
    /// Lyrics rows as built: (start ms, row index) for the tick's highlight.
    lyric_rows: Vec<(i64, usize)>,
    /// The row labels, parallel to `lyric_rows`, for retinting.
    lyric_labels: Vec<Retained<NSTextField>>,
    /// The lyrics being shown (cloned for the tick's position math).
    lyrics: Option<notchforge_core::model::Lyrics>,
    /// Highlighted lyrics row (-1 = none).
    lyric_active: isize,
    lyrics_key: u64,
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
    /// The "allow tab access" overlay shown on the browser card when the
    /// browser's Automation is off; removed and rebuilt per render.
    allow_overlay: RefCell<Option<Retained<NSButton>>>,
    state: Shared,
    tint: Retained<objc2_quartz_core::CAGradientLayer>,
    /// The cover, blurred over the whole tile (Apple Music's backdrop).
    ambient: Retained<objc2_quartz_core::CALayer>,

    art: Retained<NSImageView>,
    art_placeholder: Retained<NSImageView>,
    badge: Retained<NSTextField>,
    title: Retained<NSTextField>,
    subtitle: Retained<NSTextField>,
    album: Retained<NSTextField>,
    progress: Retained<ThinSlider>,
    elapsed: Retained<NSTextField>,
    remaining: Retained<NSTextField>,
    /// Stands in for the timeline when the source shares none.
    no_timeline: Retained<NSTextField>,
    shuffle: Retained<NSButton>,
    prev: Retained<NSButton>,
    play: Retained<NSButton>,
    next: Retained<NSButton>,
    repeat: Retained<NSButton>,
    open_spotify: Retained<NSButton>,
    like: Retained<NSButton>,
    open_music: Retained<NSButton>,

    output_button: Retained<NSButton>,
    input_button: Retained<NSButton>,
    volume: Retained<ThinSlider>,
    /// Live bars over the artwork while something is playing.
    card_eq: Retained<NSView>,

    caps: Retained<NSTextField>,
    sources: Retained<FlippedView>,
    sources_width: f64,

    seg_spotify: Retained<NSButton>,
    seg_up_next: Retained<NSButton>,
    seg_music: Retained<NSButton>,
    seg_lyrics: Retained<NSButton>,
    search_field: Retained<NSTextField>,
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

/// Jump the featured track by `delta` seconds (the ±10 s buttons).
fn skip_by(st: &Shared, delta: f64) {
    {
        let mut st = st.borrow_mut();
        let Some(n) = st.now.as_mut() else { return };
        let Some(pos) = n.position_at(now_ms()) else { return };
        let end = n.duration.unwrap_or(f64::MAX);
        let target = (pos + delta).clamp(0.0, (end - 1.0).max(0.0));
        n.position = Some(target);
        n.sampled_at_ms = now_ms();
        glue::control(n.player, MediaAction::Seek(target));
    }
    crate::app::with_controller(|c| c.media_tick());
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
        // As on the Music board: the player spans the space; the library opens
        // over it from its Library button (and closes with Done).
        let lw = w - 2.0 * PAD;
        let rx = PAD;
        let rw = lw;
        // Room at the library's top right for Done.
        const DONE_W: f64 = 70.0;
        let lib_w = 82.0;

        // ── player tile, tinted from the cover ──
        let player = super::expanded::glass_tile(mtm, rect(PAD, top_y, lw, ph));
        let tint = objc2_quartz_core::CAGradientLayer::layer();
        tint.setFrame(rect(0.0, 0.0, lw, ph));
        tint.setStartPoint(NSPoint::new(0.0, 0.0));
        tint.setEndPoint(NSPoint::new(1.0, 1.0));
        tint.setOpacity(0.0);
        let ambient = objc2_quartz_core::CALayer::new();
        ambient.setFrame(rect(-24.0, -24.0, lw + 48.0, ph + 48.0));
        ambient.setContentsGravity(&NSString::from_str("resizeAspectFill"));
        ambient.setMagnificationFilter(&NSString::from_str("trilinear"));
        ambient.setOpacity(0.0);
        // Darker toward the controls so white glyphs always read.
        let shade = objc2_quartz_core::CAGradientLayer::layer();
        shade.setFrame(rect(0.0, 0.0, lw, ph));
        set_gradient(&shade, &[(0.0, 0.0, 0.0, 0.18), (0.0, 0.0, 0.0, 0.34), (0.0, 0.0, 0.0, 0.52)]);
        if let Some(l) = player.layer() {
            l.setCornerRadius(20.0);
            l.setMasksToBounds(true);
            // As on the Music board: the player sits on the island's black,
            // with no card around it; the artwork carries the colour.
            l.setBackgroundColor(None);
            l.addSublayer(&tint);
            l.addSublayer(&ambient);
        }
        let _ = &shade;
        root.addSubview(&player);

        // The artwork takes what the rows below it leave: the timeline, the
        // transport, the volume and the sources line.
        let art_side = (ph - 200.0).clamp(64.0, 116.0);

        // ── artwork ──
        let art_shadow = FlippedView::new(mtm, rect(PAD, PAD, art_side, art_side));
        art_shadow.setWantsLayer(true);
        if let Some(l) = art_shadow.layer() {
            l.setShadowOpacity(0.5);
            l.setShadowRadius(12.0);
            l.setShadowOffset(objc2_foundation::NSSize::new(0.0, -8.0));
        }
        let art_card = shape_view(mtm, rect(0.0, 0.0, art_side, art_side), &white(0.1), 12.0);
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
        // The playing bars, pinned to the artwork's bottom edge.
        let card_eq = equalizer_view(mtm, rect(8.0, 6.0, 26.0, 12.0), &rgba(1.0, 1.0, 1.0, 0.92));
        card_eq.setHidden(true);
        art_card.addSubview(&card_eq);
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
        let tw = lw - tx - PAD - lib_w - 8.0;
        // Centred on the artwork.
        let ty = PAD + ((art_side - 78.0) / 2.0).max(0.0);
        let badge = label(mtm, "", &font(10.5, Weight::Bold), &white(0.62), rect(tx, ty, tw, 14.0));
        let title = label(mtm, "Nothing playing", &font(19.0, Weight::Bold), &p.primary, rect(tx, ty + 15.0, tw, 25.0));
        let subtitle = label(mtm, "", &font(14.0, Weight::Medium), &white(0.88), rect(tx, ty + 41.0, tw, 18.0));
        let album = label(mtm, "", &font(12.0, Weight::Regular), &white(0.6), rect(tx, ty + 60.0, tw, 16.0));
        for v in [&*badge as &NSView, &title, &subtitle, &album] {
            player.addSubview(v);
        }

        // ── timeline: the tile's full width under the artwork; it swells
        // under the pointer and shows the time being scrubbed to ──
        let bar_y = PAD + art_side + 12.0;
        let bar_w = lw - 2.0 * PAD;
        let times_y = bar_y + 20.0;
        let elapsed =
            label(mtm, "", &mono_digits(11.0, Weight::Semibold), &white(0.78), rect(PAD, times_y, 90.0, 14.0));
        let remaining = label(
            mtm,
            "",
            &mono_digits(11.0, Weight::Semibold),
            &white(0.78),
            rect(PAD + bar_w - 90.0, times_y, 90.0, 14.0),
        );
        remaining.setAlignment(NSTextAlignment::Right);
        let progress =
            ThinSlider::capsule(mtm, rect(PAD, bar_y, bar_w, 20.0), 5.0, &white(0.2), &rgba(1.0, 1.0, 1.0, 0.95));
        progress.set_grow(10.0);
        set_accessibility_label(&progress, "Playback position");
        let st = state.clone();
        let (el, rem) = (elapsed.clone(), remaining.clone());
        progress.set_on_change(move |v| {
            // Scrub preview: the clocks follow the pointer, not the track.
            let st = st.borrow();
            let Some(d) = st.now.as_ref().and_then(|n| n.duration).filter(|d| *d > 0.0) else { return };
            set_text(&el, &clock(v * d));
            set_text(&rem, &format!("-{}", clock(d - v * d)));
        });
        let no_timeline = label(
            mtm,
            "No timeline from this source",
            &font(12.0, Weight::Medium),
            &white(0.5),
            rect(PAD, bar_y + 6.0, bar_w, 16.0),
        );
        no_timeline.setAlignment(NSTextAlignment::Center);
        no_timeline.setHidden(true);
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
        for v in [&*progress as &NSView, &elapsed, &remaining, &no_timeline] {
            player.addSubview(v);
        }

        // ── transport: space-between across the tile ──
        let row_y = times_y + 18.0;
        let widths = [40.0, 48.0, 56.0, 48.0, 40.0];
        // Grouped in the middle, as on the Music board, not spread edge to edge.
        let span = (lw - 2.0 * 20.0).min(420.0);
        let gap = (span - widths.iter().sum::<f64>()) / 4.0;
        let mut xs = [0.0; 5];
        let mut x = (lw - span) / 2.0;
        for (i, bw) in widths.iter().enumerate() {
            xs[i] = x;
            x += bw + gap;
        }
        let mk = |i: usize, sym: &str, size: f64, tip: &str, action: TransportFn| {
            let st = state.clone();
            let t = targets.add(mtm, move |_| action(&st));
            let bw = widths[i];
            let bh = if i == 2 {
                56.0
            } else if i == 0 || i == 4 {
                40.0
            } else {
                44.0
            };
            let b = symbol_button(mtm, sym, size, tip, &p.primary, &t, rect(xs[i], row_y + (56.0 - bh) / 2.0, bw, bh));
            player.addSubview(&b);
            b
        };
        let shuffle = mk(
            0,
            "shuffle",
            16.0,
            "Shuffle",
            Box::new(|st| {
                let player = st.borrow().now.as_ref().map(|n| n.player);
                match player {
                    // A video has no shuffle: this slot jumps back 10 s.
                    Some(MediaPlayer::Browser) => skip_by(st, -10.0),
                    Some(pl) => {
                        let on = st.borrow().now.as_ref().and_then(|n| n.shuffle).unwrap_or(false);
                        glue::control(pl, MediaAction::SetShuffle(!on));
                    }
                    None => {}
                }
            }),
        );
        let prev = mk(
            1,
            "backward.fill",
            24.0,
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
        // The one solid control: a white disc with a dark glyph.
        make_card(&play, &white(0.96), 28.0);
        play.setContentTintColor(Some(&rgba(0.08, 0.08, 0.09, 1.0)));
        if let Some(l) = play.layer() {
            l.setShadowOpacity(0.3);
            l.setShadowRadius(8.0);
            l.setShadowOffset(objc2_foundation::NSSize::new(0.0, -3.0));
        }
        let next = mk(
            3,
            "forward.fill",
            24.0,
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
                let player = st.borrow().now.as_ref().map(|n| n.player);
                match player {
                    // …and this one forward 10 s.
                    Some(MediaPlayer::Browser) => skip_by(st, 10.0),
                    Some(pl) => {
                        let on = st.borrow().now.as_ref().and_then(|n| n.repeat).unwrap_or(false);
                        glue::control(pl, MediaAction::SetRepeat(!on));
                    }
                    None => {}
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
            rect(PAD, bar_y - 4.0, 104.0, 28.0),
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
            rect(PAD + 112.0, bar_y - 4.0, 96.0, 28.0),
        );
        player.addSubview(&open_spotify);
        player.addSubview(&open_music);
        // The heart: save the playing Spotify track (shown only then).
        let t = targets.add(mtm, move |_| {
            glue::send(ProviderCommand::SpotifyLike);
        });
        let like = symbol_button(
            mtm,
            "heart",
            13.0,
            "Save to your Spotify library",
            &p.primary,
            &t,
            rect(rw - PAD - 30.0, bar_y - 4.0, 28.0, 28.0),
        );
        make_card(&like, &white(0.14), 15.0);
        like.setHidden(true);
        player.addSubview(&like);

        // ── volume and output ──
        // ── volume: a thin bar between two speakers, then the microphone and
        // output buttons at the end ──
        let volume_y = row_y + 56.0 + 8.0;
        let output_right = lw - PAD;
        // The two round buttons (30 pt each, 6 apart) and the gap before them.
        let buttons_w = 30.0 + 6.0 + 30.0;
        let quiet = symbol_view(mtm, "speaker.fill", 11.0, &white(0.7), rect(PAD, volume_y + 7.0, 14.0, 14.0), "");
        let loud = symbol_view(
            mtm,
            "speaker.wave.3.fill",
            11.0,
            &white(0.7),
            rect(output_right - buttons_w - 12.0 - 20.0, volume_y + 7.0, 20.0, 14.0),
            "",
        );
        player.addSubview(&quiet);
        player.addSubview(&loud);
        let volume = ThinSlider::capsule(
            mtm,
            rect(PAD + 22.0, volume_y + 4.0, output_right - buttons_w - 12.0 - 26.0 - PAD - 22.0, 20.0),
            5.0,
            &white(0.2),
            &white(0.9),
        );
        volume.set_grow(10.0);
        set_accessibility_label(&volume, "Volume");
        // The featured player's own volume when it has one (Spotify, Music),
        // else the system output.
        volume.set_on_change({
            let st = state.clone();
            move |v| {
                let featured = st.borrow().featured.clone();
                if let Featured::Player(pl @ (MediaPlayer::Spotify | MediaPlayer::Music)) = featured {
                    glue::control(pl, MediaAction::SetVolume((v * 100.0).round() as u8));
                } else {
                    crate::controls::volume::set(v as f32);
                }
            }
        });
        player.addSubview(&volume);
        let st = state.clone();
        let t = targets.add(mtm, move |sender: Option<&AnyObject>| {
            let Some(b) = sender.and_then(|s| s.downcast_ref::<NSButton>()) else { return };
            output_menu(b, &st.borrow().devices);
        });
        let output_button = symbol_button(
            mtm,
            "airplayaudio",
            14.0,
            "Output",
            &p.primary,
            &t,
            rect(output_right - 30.0, volume_y - 1.0, 30.0, 30.0),
        );
        make_card(&output_button, &white(0.16), 15.0);
        player.addSubview(&output_button);
        let st = state.clone();
        let t = targets.add(mtm, move |sender: Option<&AnyObject>| {
            let Some(b) = sender.and_then(|s| s.downcast_ref::<NSButton>()) else { return };
            input_menu(b, &st.borrow().inputs);
        });
        let input_button = symbol_button(
            mtm,
            "mic.fill",
            14.0,
            "Microphone",
            &p.primary,
            &t,
            rect(output_right - 66.0, volume_y - 1.0, 30.0, 30.0),
        );
        make_card(&input_button, &white(0.16), 15.0);
        player.addSubview(&input_button);

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
            l.setCornerRadius(20.0);
            l.setMasksToBounds(true);
        }
        // Four sources across the tile's width, plus the search toggle and Done.
        let seg_w = (rw - 20.0 - DONE_W - 4.0 - 26.0) / 4.0;
        let seg_track = shape_view(mtm, rect(10.0, 10.0, rw - 20.0 - DONE_W, 28.0), &white(0.12), 14.0);
        let pick = |player: MediaPlayer| {
            let st = state.clone();
            move |_: Option<&AnyObject>| {
                {
                    let mut st = st.borrow_mut();
                    st.chosen_list_player = Some(player);
                    st.pinned = Some(player);
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
            rect(2.0, 2.0, seg_w, 24.0),
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
            rect(2.0 + seg_w, 2.0, seg_w, 24.0),
        );
        let t = targets.add(mtm, pick(MediaPlayer::Browser));
        let seg_up_next = pill_button(
            mtm,
            "Up next",
            None,
            &font(12.0, Weight::Semibold),
            &p.primary,
            &white(0.0),
            12.0,
            &t,
            rect(2.0 + 2.0 * seg_w, 2.0, seg_w, 24.0),
        );
        let t = targets.add(mtm, {
            let st = state.clone();
            move |_: Option<&AnyObject>| {
                let mut st = st.borrow_mut();
                st.show_lyrics = !st.show_lyrics;
                st.searching = false;
                st.playlists_key = 0;
                drop(st);
                crate::app::with_controller(|c| c.media_rerender());
            }
        });
        let seg_lyrics = pill_button(
            mtm,
            "Lyrics",
            None,
            &font(12.0, Weight::Semibold),
            &p.primary,
            &white(0.0),
            12.0,
            &t,
            rect(2.0 + 3.0 * seg_w, 2.0, seg_w, 24.0),
        );
        let t = targets.add(mtm, {
            let st = state.clone();
            move |_: Option<&AnyObject>| {
                let opening = {
                    let mut st = st.borrow_mut();
                    st.searching = !st.searching;
                    st.show_lyrics = false;
                    st.playlists_key = 0;
                    st.searching
                };
                crate::app::with_controller(|c| c.media_search_toggled(opening));
            }
        });
        let search_toggle = symbol_button(
            mtm,
            "magnifyingglass",
            11.0,
            "Play something from YouTube",
            &p.secondary,
            &t,
            rect(rw - 20.0 - DONE_W - 26.0 + 4.0, 8.0, 20.0, 20.0),
        );
        // The field lives beside the segments and only shows while searching.
        let search_target = crate::ui::ActionTarget::new(mtm, |_| {
            crate::app::with_controller(|c| c.media_do_search());
        });
        let search_field =
            NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(10.0, 40.0, rw - 20.0 - 34.0, 24.0));
        // SAFETY: target kept alive by the caller (Targets).
        unsafe {
            search_field.setTarget(Some(search_target.as_any()));
            search_field.setAction(Some(crate::ui::ActionTarget::action()));
        }
        search_field
            .setPlaceholderString(Some(&objc2_foundation::NSString::from_str("Search YouTube — Enter plays it here")));
        search_field.setHidden(true);
        seg_track.addSubview(&seg_spotify);
        seg_track.addSubview(&seg_music);
        seg_track.addSubview(&seg_up_next);
        seg_track.addSubview(&seg_lyrics);
        card.addSubview(&seg_track);
        card.addSubview(&search_toggle);
        card.addSubview(&search_field);

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
        // The library opens over the player and closes with Done.
        card.setHidden(true);
        root.addSubview(&card);
        let (c1, c2) = (card.clone(), card.clone());
        let t = targets.add(mtm, move |_| c1.setHidden(false));
        let library = pill_button(
            mtm,
            "Library",
            Some("music.note.list"),
            &font(12.5, Weight::Semibold),
            &p.primary,
            &white(0.14),
            15.0,
            &t,
            rect(lw - PAD - lib_w, PAD, lib_w, 30.0),
        );
        player.addSubview(&library);
        // The heart sits beside Library, clear of the progress line.
        like.setFrame(rect(lw - PAD - lib_w - 8.0 - 30.0, PAD, 30.0, 30.0));
        let t = targets.add(mtm, move |_| c2.setHidden(true));
        let done = pill_button(
            mtm,
            "Done",
            None,
            &font(12.5, Weight::Semibold),
            &p.primary,
            &white(0.14),
            14.0,
            &t,
            rect(rw - 10.0 - DONE_W + 6.0, 10.0, DONE_W - 6.0, 28.0),
        );
        card.addSubview(&done);

        Self {
            root,
            mtm,
            p: p.clone(),
            _targets: targets,
            row_targets: Targets::default(),
            source_targets: Targets::default(),
            allow_overlay: RefCell::new(None),
            state,
            tint,
            ambient,
            art,
            art_placeholder,
            badge,
            title,
            subtitle,
            album,
            progress,
            elapsed,
            remaining,
            no_timeline,
            shuffle,
            prev,
            play,
            next,
            repeat,
            open_spotify,
            like,
            open_music,
            output_button,
            input_button,
            volume,
            card_eq,
            caps,
            sources,
            sources_width,
            seg_spotify,
            seg_up_next,
            seg_music,
            seg_lyrics,
            search_field,
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
    /// The list area's height (the scroll view's frame when not searching).
    pub(super) fn list_height(&self) -> f64 {
        self.list_scroll.frame().size.height
    }

    /// Show or hide the search overlay; opening focuses the field.
    pub fn set_search_open(&self, opening: bool) {
        if !opening {
            self.close_search();
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            st.searching = true;
            st.search_results.clear();
            st.playlists_key = 0;
        }
        self.list_scroll.setFrame(rect(6.0, 76.0, self.list_width, self.list_height() - 30.0));
        self.list_scroll.setHidden(false);
        self.list_message.setHidden(true);
        self.list_action.setHidden(true);
        let field = self.search_field.clone();
        crate::app::with_controller(|c| {
            c.panel.makeFirstResponder(Some(&field));
        });
    }

    /// The text currently in the search field.
    pub fn search_query(&self) -> String {
        self.search_field.stringValue().to_string()
    }

    /// Search results came back: show them.
    pub fn set_search_results(&self, results: Vec<UpNextVideo>) {
        {
            let mut st = self.state.borrow_mut();
            st.searching = true;
            st.search_results = results;
            st.playlists_key = 0;
        }
        self.list_scroll.setHidden(false);
        self.list_message.setHidden(true);
        self.render_playlists(&notchforge_core::model::MediaState::default());
    }

    pub fn tick(&self) {
        self.tick_lyrics();
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
        let featured = self.featured(&snap.media, &snap.audio);
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

    /// The featured player with the user's pick (`pin_source`) applied: it
    /// wins over the automatic choice until it has nothing left to show.
    fn featured(&self, media: &MediaState, audio: &AudioState) -> Featured {
        let pinned = self.state.borrow().pinned;
        if let Some(pl) = pinned {
            if media.player(pl).is_some() || media.running.contains(&pl) {
                return Featured::Player(pl);
            }
            self.state.borrow_mut().pinned = None;
        }
        featured(media, audio)
    }

    /// Pin a player (sources bar / segments): the card and the list follow.
    pub fn pin(&self, player: MediaPlayer) {
        {
            let mut st = self.state.borrow_mut();
            st.pinned = Some(player);
            st.chosen_list_player = Some(player);
            st.playlists_key = 0;
        }
        crate::app::with_controller(|c| c.media_rerender());
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

    /// The cover, blurred across the tile; without one the wash shows alone.
    fn set_ambient(&self, img: Option<&objc2_app_kit::NSImage>) {
        let cg = img.and_then(icons::ambient_image);
        without_animation(|| match &cg {
            Some(cg) => {
                // SAFETY: a CGImage is a valid CALayer contents object.
                let obj = unsafe { &*(&**cg as *const objc2_core_graphics::CGImage as *const AnyObject) };
                unsafe { self.ambient.setContents(Some(obj)) };
                self.ambient.setOpacity(1.0);
            }
            None => {
                unsafe { self.ambient.setContents(None) };
                self.ambient.setOpacity(0.0);
            }
        });
    }

    fn render_featured(&self, snap: &Snapshot, featured: &Featured) {
        let p = &self.p;
        // The affordance is rebuilt on every render; drop the old one first.
        if let Some(old) = self.allow_overlay.borrow_mut().take() {
            old.removeFromSuperview();
        }
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
                    // The source website ("YouTube", a host) beats the
                    // browser's name — that is what is being watched.
                    (MediaPlayer::Browser, Some(n)) => {
                        let site = n.artist.as_str();
                        if site.is_empty() || site == "Playing audio" { n.source_name() } else { site }
                    }
                    (MediaPlayer::Browser, None) => "Browser",
                };
                set_text(&self.badge, &format!("{short}{state}").to_uppercase());
                set_text(&self.title, n.map_or("", |n| n.title.as_str()));
                let artist = n.map(|n| if n.artist.is_empty() { n.subtitle() } else { n.artist.clone() });
                set_text(&self.subtitle, &artist.unwrap_or_default());
                set_text(&self.album, n.map_or("", |n| if n.artist.is_empty() { "" } else { n.album.as_str() }));
                // The browser is audible but its tabs are off limits: offer
                // the Automation prompt right on the card.
                if let Some(np) = n {
                    let denied = np.player == MediaPlayer::Browser
                        && snap.media.browser_denied.as_deref().is_some_and(|b| np.source_bundle.as_deref() == Some(b));
                    if denied {
                        set_text(&self.subtitle, "Allow tab access to see the site and title");
                        set_accessibility_label(&self.title, "Browser tabs are private; allow access to see them");
                        let bundle = np.source_bundle.clone().unwrap_or_default();
                        let target = self.source_targets.add(self.mtm, move |_| {
                            crate::media::connect_browser(bundle.clone());
                        });
                        // SAFETY: the title's superview outlives the overlay.
                        let sup = unsafe { self.title.superview() };
                        if let Some(sup) = sup {
                            let mut f = self.title.frame();
                            let s = self.subtitle.frame();
                            let (min_y, max_y) = (f.origin.y.min(s.origin.y), f.origin.y + f.size.height);
                            let max_y = max_y.max(s.origin.y + s.size.height);
                            f.origin.x = f.origin.x.min(s.origin.x);
                            f.origin.y = min_y - 2.0;
                            f.size.width = f.size.width.max(s.size.width);
                            f.size.height = max_y - min_y + 4.0;
                            let ov = crate::ui::click_overlay(self.mtm, &target, f, "Allow tab access");
                            sup.addSubview(&ov);
                            *self.allow_overlay.borrow_mut() = Some(ov);
                        }
                    }
                }
                self.card_eq.setHidden(!n.is_some_and(NowPlaying::is_playing));
                let art = n.and_then(|n| n.artwork_path.clone());
                if self.state.borrow().art_path != art || art.is_none() {
                    let img = art.as_deref().and_then(icons::file_image);
                    self.art.setImage(img.as_deref());
                    self.art_placeholder.setHidden(img.is_some());
                    // Flat black like the rest of the island: the artwork
                    // carries the colour, the card doesn't wash with it.
                    let _ = pl;
                    self.set_wash(None);
                    self.set_ambient(None);
                    self.state.borrow_mut().art_path = art;
                }
                if let Some(n) = n {
                    let on = |b: Option<bool>| rgba(1.0, 1.0, 1.0, if b == Some(true) { 0.28 } else { 0.0 });
                    make_card(&self.shuffle, &on(n.shuffle), 20.0);
                    self.like.setHidden(*pl != MediaPlayer::Spotify);
                    make_card(&self.repeat, &on(n.repeat), 20.0);
                    self.shuffle.setContentTintColor(Some(&p.primary));
                    self.repeat.setContentTintColor(Some(&p.primary));
                    set_accessibility_label(&self.title, &format!("{}, {}", n.title, n.subtitle()));
                }
                (true, n.is_none_or(|n| n.duration.is_some()))
            }
            Featured::App { name, bundle_id, app_path } => {
                set_text(&self.badge, "PLAYING AUDIO");
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
                self.set_ambient(None);
                self.state.borrow_mut().art_path = None;
                (true, false)
            }
            Featured::Nothing => {
                set_text(&self.badge, "SILENT");
                set_text(&self.album, "");
                set_text(&self.title, "Nothing playing");
                set_text(&self.subtitle, "Start something in Spotify or Apple Music");
                self.art.setImage(None);
                self.art_placeholder.setHidden(false);
                self.set_wash(None);
                self.set_ambient(None);
                self.state.borrow_mut().art_path = None;
                (false, false)
            }
        };
        let rich = matches!(featured, Featured::Player(p) if *p != MediaPlayer::Browser);
        // A video with a timeline gets ±10 s where shuffle and repeat sit.
        let skips = matches!(featured, Featured::Player(MediaPlayer::Browser)) && has_progress;
        for b in [&self.prev, &self.play, &self.next] {
            b.setHidden(!has_controls);
        }
        if skips {
            set_button_symbol(&self.shuffle, "gobackward.10", 18.0, "Back 10 seconds");
            set_button_symbol(&self.repeat, "goforward.10", 18.0, "Forward 10 seconds");
            make_card(&self.shuffle, &rgba(1.0, 1.0, 1.0, 0.0), 20.0);
            make_card(&self.repeat, &rgba(1.0, 1.0, 1.0, 0.0), 20.0);
        } else if rich {
            set_button_symbol(&self.shuffle, "shuffle", 16.0, "Shuffle");
            set_button_symbol(&self.repeat, "repeat", 16.0, "Repeat");
        }
        self.shuffle.setHidden(!(rich || skips));
        self.repeat.setHidden(!(rich || skips));
        for v in [&*self.progress as &NSView, &self.elapsed, &self.remaining] {
            v.setHidden(!has_progress);
        }
        self.no_timeline.setHidden(has_progress || !has_controls);
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
        self.state.borrow_mut().inputs =
            audio.inputs.iter().map(|d| (d.uid.clone(), d.name.clone(), d.is_default)).collect();
        let mic = audio.inputs.iter().find(|d| d.is_default);
        set_button_symbol(
            &self.input_button,
            "mic.fill",
            14.0,
            &format!("Microphone: {}", mic.map_or("None", |d| d.name.as_str())),
        );
        self.input_button.setEnabled(!audio.inputs.is_empty());
        let name = audio.default_device().map_or("No output", |d| d.name.as_str());
        let symbol = audio.default_device().map_or("airplayaudio", |d| d.symbol());
        set_button_symbol(&self.output_button, symbol, 14.0, &format!("Output: {name}"));
        make_card(&self.output_button, &rgba(1.0, 1.0, 1.0, 0.16), 15.0);
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
        let color = rgba(1.0, 1.0, 1.0, 0.84);
        let f = font(12.0, Weight::Regular);
        let width = self.sources_width;
        let text_at = |x: f64, text: &str| {
            self.sources.addSubview(&label(self.mtm, text, &f, &color, rect(x, 4.0, (width - x).max(20.0), 16.0)));
        };
        if !audio.sources_supported {
            text_at(0.0, "Per-app audio needs macOS 14.2 or later");
            return;
        }
        if audio.sources.is_empty() {
            text_at(0.0, "No app is playing audio");
            return;
        }
        // The bar: every playing source with its icon; the featured one is
        // bright, the rest are one click away — tapping one switches what the
        // card and the transport controls below act on.
        let mut x = 0.0;
        let mut shown = String::new();
        for s in &audio.sources {
            let mapped = s.bundle_id.as_deref().and_then(MediaPlayer::from_bundle_id);
            let is_featured = mapped.is_some_and(|pl| matches!(&featured, Featured::Player(f) if *f == pl));
            let entry_color = if is_featured { rgba(1.0, 1.0, 1.0, 1.0) } else { rgba(1.0, 1.0, 1.0, 0.62) };
            let slot = rect(x, 3.0, 18.0, 18.0);
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
                icon.setContentTintColor(Some(&entry_color));
                self.sources.addSubview(&icon);
            }
            let icon_end = x + 22.0;
            let text = if is_featured { format!("{} · on {}", s.name, audio.route_label(s)) } else { s.name.clone() };
            let tw = text_width(&attributed(&text, &f, &entry_color)) + 6.0;
            if icon_end + tw > width {
                break;
            }
            let lbl = label(self.mtm, &text, &f, &entry_color, rect(icon_end, 4.0, tw, 16.0));
            self.sources.addSubview(&lbl);
            match mapped {
                Some(player) if !is_featured => {
                    let t = self.source_targets.add(self.mtm, move |_| glue::pin_source(player));
                    let overlay =
                        click_overlay(self.mtm, &t, rect(x, 0.0, 22.0 + tw, 24.0), &format!("Control {}", s.name));
                    // Spotify and Music answer their own volume: right-click
                    // offers a slider. Other apps have no public per-app
                    // volume on macOS, so nothing appears for them.
                    if matches!(player, MediaPlayer::Spotify | MediaPlayer::Music) {
                        let t = self.source_targets.add(self.mtm, move |sender: Option<&AnyObject>| {
                            let Some(sl) = sender.and_then(|sl| sl.downcast_ref::<NSSlider>()) else { return };
                            let v = sl.doubleValue().round() as u8;
                            glue::control(player, MediaAction::SetVolume(v));
                        });
                        // SAFETY: valid target/selector on a standard slider.
                        let slider = unsafe {
                            NSSlider::sliderWithTarget_action(
                                Some(t.as_any()),
                                Some(crate::ui::ActionTarget::action()),
                                self.mtm,
                            )
                        };
                        slider.setMinValue(0.0);
                        slider.setMaxValue(100.0);
                        slider.setFrame(rect(0.0, 6.0, 200.0, 18.0));
                        let label_text = format!("{} volume", s.name);
                        let lbl = label(
                            self.mtm,
                            &label_text,
                            &font(11.0, Weight::Regular),
                            &rgba(1.0, 1.0, 1.0, 0.84),
                            rect(0.0, 26.0, 200.0, 14.0),
                        );
                        let holder = FlippedView::new(self.mtm, rect(0.0, 0.0, 220.0, 44.0));
                        holder.addSubview(&lbl);
                        holder.addSubview(&slider);
                        let vol_item = NSMenuItem::new(self.mtm);
                        vol_item.setView(Some(&holder));
                        let vol_menu = NSMenu::new(self.mtm);
                        vol_menu.addItem(&vol_item);
                        // SAFETY: the menu's targets are kept alive by `source_targets`.
                        unsafe { overlay.setMenu(Some(&vol_menu)) };
                    }
                    self.sources.addSubview(&overlay);
                }
                // Not a known player (or already featured): the click just
                // brings its app forward.
                _ => {
                    if let Some(b) = s.bundle_id.clone() {
                        let t = self.source_targets.add(self.mtm, move |_| glue::activate_app(&b));
                        self.sources.addSubview(&click_overlay(
                            self.mtm,
                            &t,
                            rect(x, 0.0, 22.0 + tw, 24.0),
                            &format!("Show {}", s.name),
                        ));
                    }
                }
            }
            if !shown.is_empty() {
                shown.push_str(", ");
            }
            shown.push_str(&s.name);
            x = icon_end + tw + 14.0;
        }
        set_accessibility_label(&self.sources, &shown);
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
fn input_menu(anchor: &NSButton, devices: &[(String, String, bool)]) {
    let mtm = MainThreadMarker::from(anchor);
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    // Targets must outlive the menu's tracking; the menu is modal, so a local
    // store dropped after `popUp…` returns is enough.
    let targets = Targets::default();
    for (uid, name, is_default) in devices {
        let uid = uid.clone();
        let t = targets.add(mtm, move |_| glue::set_input(&uid));
        let item = NSMenuItem::new(mtm);
        item.setTitle(&NSString::from_str(name));
        item.setState(if *is_default {
            objc2_app_kit::NSControlStateValueOn
        } else {
            objc2_app_kit::NSControlStateValueOff
        });
        item.setEnabled(!*is_default);
        // SAFETY: valid target/selector; the target lives in `targets`.
        unsafe {
            item.setTarget(Some(t.as_any()));
            item.setAction(Some(crate::ui::ActionTarget::action()));
        }
        menu.addItem(&item);
    }
    menu.popUpMenuPositioningItem_atLocation_inView(
        None,
        NSPoint::new(0.0, anchor.frame().size.height + 4.0),
        Some(anchor),
    );
}

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
