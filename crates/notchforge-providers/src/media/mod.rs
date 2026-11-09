//! Media provider: Spotify and Apple Music — now playing, cover art, transport
//! controls and playlists.
//!
//! * **Now playing** comes from each player's AppleScript dictionary
//!   ([`script`]), read only while the player is running (checked with a
//!   process lookup, so we never launch a player by asking about it). The macOS
//!   layer forwards the players' own change announcements
//!   (`com.spotify.client.PlaybackStateChanged`, `com.apple.Music.playerInfo`)
//!   as [`ProviderCommand::MediaChanged`], so the poll can stay slow.
//! * **Browser audio** ([`browser`]): a browser that is making sound is shown
//!   as a track too, titled from its tabs.
//! * **Cover art** is cached under `artwork/`: Spotify's artwork URL is
//!   downloaded; Music's embedded artwork is exported by a script.
//! * **Playlists**: Music via AppleScript; Spotify via the Web API
//!   ([`spotify`]) once the user has signed in. Loading runs in a background
//!   task so controls stay responsive.

pub mod browser;
pub mod script;
pub mod lrclib;
pub mod spotify;
pub mod youtube;

use notchforge_core::config::Paths;
use notchforge_core::model::{
    MediaAction, MediaPlayer, MediaState, NowPlaying, PlaybackState, Playlist, PlaylistStatus,
};
use notchforge_core::model::{Permission, ProviderStatus};
use notchforge_core::provider::{
    BoxFuture, Provider, ProviderCommand, ProviderContext, ProviderDescriptor, ProviderError,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct MediaProvider;

/// Poll while no player runs (just to notice one starting).
const IDLE_POLL: Duration = Duration::from_secs(30);
/// Let the player apply a command before reading it back.
const SETTLE: Duration = Duration::from_millis(250);
/// Don't reload playlists more often than this on page opens.
const PLAYLIST_TTL_MS: i64 = 60_000;
/// Cover art files kept on disk.
const ARTWORK_KEEP: usize = 160;

/// 64-bit FNV-1a, for cache file names.
pub(crate) fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// Cache key identifying a track's artwork.
pub fn track_key(n: &NowPlaying) -> String {
    match &n.track_id {
        Some(id) => format!("{:?}:{id}", n.player),
        None => format!("{:?}:{}|{}|{}", n.player, n.title, n.artist, n.album),
    }
}

/// Keep the newest `keep` files in `dir`.
pub fn prune_dir(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> =
        rd.flatten().filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    if files.len() <= keep {
        return;
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, p) in files.into_iter().skip(keep) {
        let _ = std::fs::remove_file(p);
    }
}

/// Download a small file into the artwork cache (skipped when it exists).
/// Best effort: `None` on any failure. Shared with the other providers
/// (platform favicons).
pub(crate) async fn download(http: &reqwest::Client, url: &str, dest: &Path) -> Option<PathBuf> {
    if dest.exists() {
        return Some(dest.to_owned());
    }
    let resp = http.get(url).send().await.ok()?.error_for_status().ok()?;
    let bytes = resp.bytes().await.ok()?;
    if bytes.is_empty() || bytes.len() > 4 << 20 {
        return None;
    }
    let tmp = dest.with_extension("part");
    tokio::fs::write(&tmp, &bytes).await.ok()?;
    tokio::fs::rename(&tmp, dest).await.ok()?;
    Some(dest.to_owned())
}

/// Download playlist covers (best effort) into the artwork cache.
async fn fetch_playlist_art(http: &reqwest::Client, dir: &Path, lists: &mut [Playlist]) {
    for p in lists.iter_mut().take(80) {
        if let Some(url) = p.artwork_url.clone() {
            let dest = dir.join(format!("pl-{:016x}.jpg", fnv(&url)));
            p.artwork_path = download(http, &url, &dest).await;
        }
    }
}

type PlaylistResult = (MediaPlayer, PlaylistStatus, Vec<Playlist>);

/// Why the provider loop woke up.
enum Wake {
    Tick,
    Playlists(PlaylistResult),
    Login(Result<Result<(), spotify::SpotifyError>, tokio::task::JoinError>),
    AudioChanged,
    Command(ProviderCommand),
}

/// Load one player's playlists (runs in a background task).
async fn load_playlists(
    player: MediaPlayer,
    http: reqwest::Client,
    paths: std::sync::Arc<Paths>,
    max: usize,
) -> PlaylistResult {
    match player {
        MediaPlayer::Browser => (player, PlaylistStatus::Loaded, vec![]),
        MediaPlayer::Music => match script::run(script::MUSIC_PLAYLISTS_SCRIPT, &[&max.to_string()]).await {
            Ok(out) => (player, PlaylistStatus::Loaded, script::parse_music_playlists(&out)),
            Err(script::ScriptError::NotPermitted) => (player, PlaylistStatus::NeedsAutomation, vec![]),
            Err(e) => (player, PlaylistStatus::Failed(e.to_string()), vec![]),
        },
        MediaPlayer::Spotify => {
            let now = notchforge_core::time::now_ms();
            match spotify::access_token(&http, &paths.spotify_token(), now).await {
                Ok(token) => match spotify::playlists(&http, &token, max).await {
                    Ok(mut lists) => {
                        let dir = paths.artwork_dir();
                        let _ = tokio::fs::create_dir_all(&dir).await;
                        fetch_playlist_art(&http, &dir, &mut lists).await;
                        (player, PlaylistStatus::Loaded, lists)
                    }
                    Err(e) => (player, PlaylistStatus::Failed(e.to_string()), vec![]),
                },
                Err(spotify::SpotifyError::NotSignedIn) => (player, PlaylistStatus::NeedsSpotifyLogin, vec![]),
                Err(e) => (player, PlaylistStatus::Failed(e.to_string()), vec![]),
            }
        }
    }
}

/// Merge one player's playlist result into the state.
pub fn apply_playlists(state: &mut MediaState, (player, status, lists): PlaylistResult) {
    // Keep the previous list on transient failures.
    if matches!(status, PlaylistStatus::Failed(_)) && state.playlists_for(player).next().is_some() {
        tracing::warn!(?player, ?status, "playlist reload failed; keeping previous list");
    } else {
        state.playlists.retain(|p| p.player != player);
        state.playlists.extend(lists);
    }
    state.playlist_status.retain(|(p, _)| *p != player);
    state.playlist_status.push((player, status));
    state.playlist_status.sort_by_key(|(p, _)| *p);
}

#[cfg(target_os = "macos")]
fn is_running(player: MediaPlayer) -> bool {
    match player {
        MediaPlayer::Spotify => crate::sys::process::is_app_running("Spotify", "Spotify.app/Contents/MacOS/Spotify"),
        MediaPlayer::Music => crate::sys::process::is_app_running("Music", "Music.app/Contents/MacOS/Music"),
        MediaPlayer::Browser => false,
    }
}

#[cfg(not(target_os = "macos"))]
fn is_running(_player: MediaPlayer) -> bool {
    false
}

struct Session {
    http: reqwest::Client,
    paths: std::sync::Arc<Paths>,
    state: MediaState,
    /// Artwork per track key, so a track's art is fetched once.
    artwork: std::collections::HashMap<String, Option<PathBuf>>,
    playlists_loaded_ms: std::collections::HashMap<MediaPlayer, i64>,
    playlist_tx: tokio::sync::mpsc::Sender<PlaylistResult>,
    loading: std::collections::HashSet<MediaPlayer>,
    /// The last browser track, kept as "paused" for a while after the sound stops.
    last_browser: Option<(NowPlaying, std::time::Instant)>,
    /// A browser that is audible but whose tabs Automation hides.
    browser_denied: Option<String>,
    /// One-shot markers so tab-reading trouble is logged at INFO exactly once.
    browser_fail_logged: bool,
    browser_ax_logged: bool,
    /// The YouTube video whose recommendations are in `state.up_next`.
    up_next_for: Option<String>,
    /// Last time the Spotify play queue was refreshed (throttled).
    queue_at: Option<std::time::Instant>,
    /// The window-title query last sent to YouTube's search, with the video
    /// it resolved to: a title is searched once, then reused until the title
    /// changes.
    yt_resolved: Option<(String, youtube::UpNextVideo)>,
    /// The browser currently making sound: navigation targets its tab.
    browser_bundle: Option<&'static str>,
    /// The track lyrics were last fetched for (`title|artist`).
    lyrics_for: Option<String>,
    /// The playing page's timeline (browser pid, element), found once via
    /// Accessibility and read on every poll.
    web_slider: Option<(i32, crate::sys::ax::WebSlider)>,
    /// When the last timeline search came up empty, and for which track:
    /// searching is a walk of the page, so a miss waits before retrying.
    web_slider_miss: Option<(std::time::Instant, Option<String>)>,
    /// The last raw timeline reading and the position it stands for, with
    /// when that was true: YouTube stops updating its slider while the
    /// controls are hidden, so an unchanged reading keeps counting from here.
    web_last: Option<WebReading>,
}

#[derive(Debug, Clone, Copy)]
struct WebReading {
    raw: (f64, f64),
    pos: f64,
    at_ms: i64,
}

/// How long a silent browser tab stays on the notch as a paused track.
const BROWSER_LINGER: Duration = Duration::from_secs(120);

impl Session {
    async fn artwork_for(&mut self, n: &NowPlaying, script_out: &str) -> Option<PathBuf> {
        let key = track_key(n);
        if let Some(Some(p)) = self.artwork.get(&key) {
            if p.exists() {
                return Some(p.clone());
            }
        } else if let Some(None) = self.artwork.get(&key) {
            return None;
        }
        let dir = self.paths.artwork_dir();
        let _ = tokio::fs::create_dir_all(&dir).await;
        let path = match n.player {
            MediaPlayer::Browser => match n.track_id.as_deref().and_then(browser::youtube_id) {
                Some(id) => {
                    let url = format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg");
                    download(&self.http, &url, &dir.join(format!("yt-{id}.jpg"))).await
                }
                // No video thumbnail: the site's favicon stands in — for any
                // site with a URL, or a known platform matched by title.
                None => self.site_icon_for(n).await,
            },
            MediaPlayer::Spotify => match script::artwork_url(script_out) {
                Some(url) => download(&self.http, &url, &dir.join(format!("sp-{:016x}.jpg", fnv(&url)))).await,
                None => None,
            },
            MediaPlayer::Music => {
                let dest = dir.join(format!("am-{:016x}.img", fnv(&key)));
                if dest.exists() {
                    Some(dest)
                } else {
                    let arg = dest.to_string_lossy().into_owned();
                    match script::run(script::MUSIC_ARTWORK_SCRIPT, &[&arg]).await {
                        Ok(o) if o.trim() == "ok" && dest.exists() => Some(dest),
                        _ => None,
                    }
                }
            }
        };
        if self.artwork.len() > 256 {
            self.artwork.clear();
        }
        self.artwork.insert(key, path.clone());
        prune_dir(&dir, ARTWORK_KEEP);
        path
    }

    /// The favicon of the site a browser track belongs to. Known platforms
    /// cache under their stable key (`site-youtube.jpg`) — the same file the
    /// notification path uses — and any other site under its host's hash.
    async fn site_icon_for(&mut self, n: &NowPlaying) -> Option<PathBuf> {
        use notchforge_core::sites;
        let host = n.track_id.as_deref().map(browser::host).filter(|h| !h.is_empty());
        let site = host.and_then(sites::site_for_host).or_else(|| sites::site_from_text(&n.title));
        let (key, domain) = match (site, host) {
            (Some(s), _) => (s.key.to_owned(), s.hosts[0].to_owned()),
            (None, Some(h)) => (format!("host-{:016x}", fnv(h)), h.to_owned()),
            (None, None) => return None,
        };
        let dir = self.paths.artwork_dir();
        let _ = tokio::fs::create_dir_all(&dir).await;
        let url = format!("https://www.google.com/s2/favicons?domain={domain}&sz=128");
        download(&self.http, &url, &dir.join(format!("site-{key}.jpg"))).await
    }

    /// Fetch lyrics for the featured track, once per track (LRCLIB has no
    /// quota but there is no point refetching either). `None` clears when
    /// nothing is playing.
    async fn refresh_lyrics(&mut self) {
        let Some(n) = self.state.primary().cloned() else {
            if self.state.lyrics.is_some() {
                self.state.lyrics = None;
            }
            self.lyrics_for = None;
            return;
        };
        let artist = if n.artist.is_empty() { n.subtitle() } else { n.artist.clone() };
        // A browser track's "artist" is a site name — search the title alone.
        let artist = if artist.eq_ignore_ascii_case("playing audio")
            || artist.to_lowercase().contains("youtube")
            || artist.is_empty()
        {
            String::new()
        } else {
            artist
        };
        let key = format!("{}|{artist}", n.title);
        if self.lyrics_for.as_deref() == Some(key.as_str()) {
            return;
        }
        if n.title.trim().is_empty() || n.title.starts_with("Playing from") {
            self.state.lyrics = None;
            self.lyrics_for = None;
            return;
        }
        self.lyrics_for = Some(key);
        self.state.lyrics = lrclib::fetch(&self.http, &n.title, &artist).await;
    }

    /// Refresh Spotify's play queue, at most every 30 s and only while
    /// Spotify runs with a stored sign-in.
    async fn refresh_queue(&mut self) {
        const QUEUE_TTL: Duration = Duration::from_secs(30);
        if self.queue_at.is_some_and(|t| t.elapsed() < QUEUE_TTL) {
            return;
        }
        self.queue_at = Some(std::time::Instant::now());
        if !self.state.running.contains(&MediaPlayer::Spotify) {
            if !self.state.spotify_queue.is_empty() {
                self.state.spotify_queue.clear();
            }
            return;
        }
        let now = notchforge_core::time::now_ms();
        let Ok(token) = spotify::access_token(&self.http, &self.paths.spotify_token(), now).await else {
            self.state.spotify_queue.clear(); // not signed in
            return;
        };
        match spotify::queue(&self.http, &token).await {
            Ok(q) => self.state.spotify_queue = q,
            Err(e) => tracing::debug!(error = %e, "could not read the Spotify queue"),
        }
    }

    /// Read every running player. Returns players that just started running.
    async fn read_players(&mut self, browser_audio: bool) -> Vec<MediaPlayer> {
        let mut players = Vec::new();
        let mut running = Vec::new();
        let mut denied = Vec::new();
        for player in MediaPlayer::ALL {
            if !tokio::task::spawn_blocking(move || is_running(player)).await.unwrap_or(false) {
                continue;
            }
            running.push(player);
            match script::run(&script::state_script(player), &[]).await {
                Ok(out) => {
                    let now = notchforge_core::time::now_ms();
                    if let Some(mut np) = script::parse_now_playing(player, &out, now) {
                        np.artwork_path = self.artwork_for(&np, &out).await;
                        players.push(np);
                    }
                }
                Err(script::ScriptError::NotPermitted) => denied.push(player),
                Err(script::ScriptError::NotRunning) => running.retain(|p| *p != player),
                Err(e) => tracing::debug!(?player, error = %e, "could not read player"),
            }
        }
        if browser_audio {
            if let Some(np) = self.read_browser().await {
                players.push(np);
            }
        } else {
            self.last_browser = None;
        }
        let started = running.iter().copied().filter(|p| !self.state.running.contains(p)).collect();
        self.state.players = players;
        self.state.running = running;
        self.state.automation_denied = denied;
        self.state.browser_denied = self.browser_denied.clone();
        started
    }

    /// The playing page's position and length, from its timeline slider in
    /// the Accessibility tree. The element is cached; a miss is retried after
    /// a while or when the track changes.
    async fn read_timeline(&mut self, pid: i32, np: &NowPlaying) -> Option<(f64, f64)> {
        const RETRY: Duration = Duration::from_secs(15);
        let cached = match self.web_slider.take() {
            Some((p, s)) if p == pid => Some(s),
            _ => None,
        };
        let may_search = match &self.web_slider_miss {
            Some((at, track)) => at.elapsed() > RETRY || *track != np.track_id,
            None => true,
        };
        let (slider, reading) = tokio::task::spawn_blocking(move || {
            if let Some(s) = cached {
                if let Some(r) = s.read() {
                    return (Some(s), Some(r));
                }
            }
            if !may_search {
                return (None, None);
            }
            let s = crate::sys::ax::WebSlider::find(pid);
            let r = s.as_ref().and_then(|s| s.read());
            (s, r)
        })
        .await
        .unwrap_or((None, None));
        match slider {
            Some(s) if reading.is_some() => {
                self.web_slider = Some((pid, s));
                self.web_slider_miss = None;
            }
            _ => {
                if may_search {
                    self.web_slider_miss = Some((std::time::Instant::now(), np.track_id.clone()));
                }
            }
        }
        reading
    }

    /// Move the playing browser tab to `secs`: page JavaScript where the
    /// browser allows it, else the timeline element itself, else (YouTube)
    /// the video's own `&t=` link in the same tab.
    async fn seek_browser(&mut self, secs: f64) {
        let secs = secs.max(0.0);
        let Some(bundle) = self.browser_bundle else { return };
        // Whatever moves it, the page now stands at `secs`.
        if let Some(r) = self.web_last.as_mut() {
            r.pos = secs;
            r.at_ms = notchforge_core::time::now_ms();
        }
        let family = browser::BROWSERS.iter().find(|b| b.bundle_id == bundle).map(|b| b.family);
        if matches!(family, Some(browser::Family::Chromium | browser::Family::Safari)) {
            let src = script::seek_script(bundle, family == Some(browser::Family::Safari));
            let arg = format!("{secs:.2}");
            if script::run(&src, &[arg.as_str()]).await.is_ok_and(|o| o.trim() == "ok") {
                tracing::debug!("browser seek: page script");
                return;
            }
        }
        if let Some((pid, s)) = self.web_slider.take() {
            let moved = tokio::task::spawn_blocking(move || {
                let ok = s.set(secs) || s.step_to(secs);
                (s, ok)
            })
            .await;
            if let Ok((s, ok)) = moved {
                self.web_slider = Some((pid, s));
                if ok {
                    tracing::debug!("browser seek: timeline element");
                    return;
                }
            }
        }
        let video = self.last_browser.as_ref().and_then(|(np, _)| np.track_id.as_deref().and_then(browser::youtube_id));
        if let Some(id) = video {
            let url = format!("https://www.youtube.com/watch?v={id}&t={}s", secs.round() as i64);
            tracing::debug!("browser seek: reopening the video at the time");
            let src = script::navigate_script(bundle);
            let _ = script::run(&src, &[url.as_str()]).await;
        }
    }

    /// A browser that is making sound, as a track (or the one it just played).
    async fn read_browser(&mut self) -> Option<NowPlaying> {
        let now = notchforge_core::time::now_ms();
        let sources = tokio::task::spawn_blocking(crate::audio::playing_sources).await.unwrap_or_default();
        if let Some((b, src)) = browser::find(&sources) {
            self.browser_bundle = Some(b.bundle_id);
            let mut denied = false;
            let mut tabs = match browser::tabs_script(b) {
                Some(src) => match script::run(&src, &[]).await {
                    Ok(out) => {
                        self.browser_fail_logged = false;
                        browser::parse_tabs(&out)
                    }
                    Err(script::ScriptError::NotPermitted) => {
                        tracing::info!(browser = b.name, "browser tab reading needs Automation");
                        denied = true;
                        vec![]
                    }
                    Err(e) => {
                        // Once at INFO: this failure is why the card shows no
                        // video title, so it must not hide at DEBUG level.
                        if !self.browser_fail_logged {
                            tracing::info!(browser = b.name, error = %e, "cannot read browser tabs; using window titles");
                            self.browser_fail_logged = true;
                        } else {
                            tracing::debug!(browser = b.name, error = %e, "could not read browser tabs");
                        }
                        vec![]
                    }
                },
                None => vec![],
            };
            // Fallback: the windows' titles via Accessibility — a different
            // permission than AppleScript Automation, and one that works for
            // every browser (Firefox included): the tab title is the window
            // title.
            if tabs.is_empty() {
                // The audio pid is a browser helper; the windows live in the
                // browser's own (responsible) process. The page's own <title>
                // (via the AX tree) beats the window title, which macOS
                // truncates.
                let pid = crate::sys::process::responsible_pid(src.pid);
                let mut titles =
                    tokio::task::spawn_blocking(move || crate::sys::ax::web_titles(pid)).await.unwrap_or_default();
                if titles.is_empty() {
                    titles = tokio::task::spawn_blocking(move || crate::sys::ax::window_titles(pid))
                        .await
                        .unwrap_or_default();
                }
                if !titles.is_empty() && !self.browser_ax_logged {
                    tracing::info!(browser = b.name, count = titles.len(), "using window titles as browser tabs");
                    self.browser_ax_logged = true;
                }
                tabs = titles.into_iter().map(|title| browser::Tab { title, url: String::new() }).collect();
            }
            self.browser_denied = denied.then(|| b.name.to_owned());
            // A YouTube video whose URL we never saw is found by its title:
            // that fills the name, the thumbnail and the up-next. The title
            // comes from the tab when it has one, else from the page's own
            // `<title>` in the Accessibility tree — feed autoplay (the video
            // plays right on youtube.com) never changes the tab's URL.
            let mut picked = browser::pick(&tabs).cloned();
            let mut resolved_author: Option<String> = None;
            let page_titles = if picked.as_ref().is_some_and(|t| browser::youtube_id(&t.url).is_none())
                && picked.as_ref().and_then(|t| browser::youtube_title_query(&t.title)).is_none()
            {
                // The audio source's pid is a helper; the real page lives in
                // the browser's own (responsible) process.
                let main_pid = crate::sys::process::responsible_pid(src.pid);
                tokio::task::spawn_blocking(move || crate::sys::ax::web_titles(main_pid)).await.unwrap_or_default()
            } else {
                Vec::new()
            };
            if let Some(t) = picked.as_mut() {
                let title_query = match browser::youtube_title_query(&t.title) {
                    Some(q) => Some(q),
                    None => page_titles.iter().find_map(|t| browser::youtube_title_query(t)),
                };
                if browser::youtube_id(&t.url).is_none() {
                    match title_query {
                        Some(query) => {
                            let found = match &self.yt_resolved {
                                Some((q, v)) if *q == query => Some(v.clone()),
                                _ => {
                                    let v = youtube::search_first(&self.http, &query).await;
                                    if let Some(found) = &v {
                                        tracing::info!(from = %t.title, found = %found.title, "resolved the YouTube video from its title");
                                    }
                                    self.yt_resolved = v.clone().map(|v| (query.clone(), v.clone()));
                                    v
                                }
                            };
                            if let Some(v) = found {
                                resolved_author = (!v.author.is_empty()).then(|| v.author.clone());
                                t.title = v.title;
                                t.url = v.url;
                            }
                        }
                        // Not YouTube after all (or a bare feed): forget it.
                        None => self.yt_resolved = None,
                    }
                }
            }
            let mut np = browser::now_playing(b, picked.as_ref(), PlaybackState::Playing, now);
            if let Some(author) = resolved_author {
                np.artist = author;
            }
            np.artwork_path = self.artwork_for(&np, "").await;
            if let Some((pos, dur)) = self.read_timeline(crate::sys::process::responsible_pid(src.pid), &np).await {
                let now = notchforge_core::time::now_ms();
                let r = match self.web_last {
                    // Same text as before: stale, so count on from the
                    // position it was last known to mean.
                    Some(last) if last.raw == (pos, dur) => last,
                    _ => WebReading { raw: (pos, dur), pos, at_ms: now },
                };
                self.web_last = Some(r);
                np.duration = Some(dur);
                np.position = Some(r.pos);
                np.sampled_at_ms = r.at_ms;
            } else {
                self.web_last = None;
            }
            // YouTube's up-next recommendations: fetched once per video.
            let video_id = np.track_id.as_deref().and_then(browser::youtube_id);
            match &video_id {
                Some(id) if self.up_next_for.as_deref() == Some(id) => {}
                Some(id) => {
                    self.up_next_for = Some(id.clone());
                    self.state.up_next = youtube::fetch_up_next(&self.http, id).await;
                    if self.state.up_next.is_empty() {
                        self.up_next_for = None; // retry next poll (offline?)
                    } else {
                        tracing::info!(count = self.state.up_next.len(), "fetched YouTube up-next");
                    }
                }
                None => {
                    self.up_next_for = None;
                    self.state.up_next = Vec::new();
                }
            }
            self.last_browser = Some((np.clone(), std::time::Instant::now()));
            return Some(np);
        }
        self.browser_denied = None;
        match &self.last_browser {
            Some((np, at)) if at.elapsed() < BROWSER_LINGER => {
                Some(NowPlaying { state: PlaybackState::Paused, sampled_at_ms: np.sampled_at_ms, ..np.clone() })
            }
            _ => {
                self.last_browser = None;
                self.up_next_for = None;
                self.state.up_next = Vec::new();
                None
            }
        }
    }

    fn reload_playlists(&mut self, player: MediaPlayer, ctx: &ProviderContext, force: bool) {
        let now = notchforge_core::time::now_ms();
        let fresh = self.playlists_loaded_ms.get(&player).is_some_and(|t| now - t < PLAYLIST_TTL_MS);
        if self.loading.contains(&player) || (fresh && !force) {
            return;
        }
        // Music playlists need the app running (asking would launch it).
        if player == MediaPlayer::Music && !self.state.running.contains(&player) {
            return;
        }
        let cfg = ctx.config();
        self.loading.insert(player);
        self.playlists_loaded_ms.insert(player, now);
        let tx = self.playlist_tx.clone();
        let fut = load_playlists(player, self.http.clone(), self.paths.clone(), cfg.media.max_playlists);
        tokio::spawn(async move {
            let _ = tx.send(fut.await).await;
        });
    }

    async fn control(&mut self, player: MediaPlayer, action: &MediaAction) {
        // Only "play this playlist" may launch a player; other controls are
        // meaningless when it is not running.
        if player == MediaPlayer::Browser {
            // Navigate the tab that is already playing; a browser we cannot
            // reach falls back to opening the URL. Other browser controls are
            // media keys, sent by the macOS layer.
            if let MediaAction::Seek(secs) = action {
                self.seek_browser(*secs).await;
                return;
            }
            if let MediaAction::Navigate(url) = action.clone() {
                let navigated = match self.browser_bundle {
                    Some(bundle) => {
                        let src = script::navigate_script(bundle);
                        script::run(&src, &[url.as_str()]).await.is_ok()
                    }
                    None => false,
                };
                if !navigated {
                    tracing::info!("cannot reach the playing tab; opening the URL instead");
                    let _ = tokio::task::spawn_blocking(move || {
                        let _ = std::process::Command::new("open").arg(url).status();
                    })
                    .await;
                }
            }
            return;
        }
        let launches = matches!(action, MediaAction::PlayPlaylist(_));
        if !launches && !tokio::task::spawn_blocking(move || is_running(player)).await.unwrap_or(false) {
            return;
        }
        let (src, args) = script::control(player, action);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        if let Err(e) = script::run(&src, &args).await {
            tracing::warn!(?player, ?action, error = %e, "media control failed");
            if e == script::ScriptError::NotPermitted && !self.state.automation_denied.contains(&player) {
                self.state.automation_denied.push(player);
            }
        }
    }
}

impl Provider for MediaProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor { id: "media", name: "Spotify & Apple Music", permissions: &[Permission::Automation] }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            if !cfg!(target_os = "macos") {
                ctx.set_status(ProviderStatus::Failing("Spotify and Apple Music are only available on macOS".into()))
                    .await;
                ctx.shutdown.wait().await;
                return Ok(());
            }
            let http = reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent(concat!("NotchForge/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(ProviderError::other)?;
            let (playlist_tx, mut playlist_rx) = tokio::sync::mpsc::channel::<PlaylistResult>(8);
            let mut s = Session {
                http,
                paths: ctx.paths.clone(),
                state: MediaState::default(),
                artwork: Default::default(),
                playlists_loaded_ms: Default::default(),
                playlist_tx,
                loading: Default::default(),
                last_browser: None,
                browser_denied: None,
                browser_fail_logged: false,
                browser_ax_logged: false,
                up_next_for: None,
                queue_at: None,
                yt_resolved: None,
                browser_bundle: None,
                lyrics_for: None,
                web_slider: None,
                web_slider_miss: None,
                web_last: None,
            };
            let mut login: Option<tokio::task::JoinHandle<Result<(), spotify::SpotifyError>>> = None;
            let mut last_emitted: Option<MediaState> = None;
            let mut first = true;
            ctx.set_status(ProviderStatus::Running).await;

            loop {
                s.refresh_queue().await;
                let started = s.read_players(ctx.config().media.browser_audio).await;
                s.refresh_lyrics().await;
                for p in started {
                    s.reload_playlists(p, &ctx, true);
                }
                if first {
                    s.reload_playlists(MediaPlayer::Spotify, &ctx, true);
                    first = false;
                }
                if last_emitted.as_ref() != Some(&s.state) {
                    if !ctx.emit(notchforge_core::provider::ProviderEvent::Media(s.state.clone())).await {
                        return Ok(());
                    }
                    let denied = !s.state.automation_denied.is_empty();
                    let was_denied = last_emitted.as_ref().is_some_and(|m| !m.automation_denied.is_empty());
                    if denied != was_denied || last_emitted.is_none() {
                        ctx.set_status(if denied {
                            ProviderStatus::NeedsPermission(Permission::Automation)
                        } else {
                            ProviderStatus::Running
                        })
                        .await;
                    }
                    last_emitted = Some(s.state.clone());
                }

                // A browser track has no change announcements of its own: keep
                // polling at the configured rate so tab switches surface.
                let poll = if s.state.running.is_empty()
                    && !s.state.players.iter().any(|p| p.player == MediaPlayer::Browser)
                {
                    IDLE_POLL
                } else {
                    Duration::from_secs(ctx.config().media.poll_seconds)
                };
                let refresh = ctx.refresh.clone();
                let login_done = async {
                    match login.as_mut() {
                        Some(h) => h.await,
                        None => std::future::pending().await,
                    }
                };
                let browser_audio = ctx.config().media.browser_audio;
                let audio_changed = async {
                    if browser_audio { crate::audio::wait_changed().await } else { std::future::pending().await }
                };
                let next = tokio::select! {
                    _ = tokio::time::sleep(poll) => Wake::Tick,
                    _ = audio_changed => Wake::AudioChanged,
                    _ = refresh.notified() => Wake::Tick,
                    Some(result) = playlist_rx.recv() => Wake::Playlists(result),
                    r = login_done => Wake::Login(r),
                    cmd = ctx.next_command() => match cmd {
                        Some(c) => Wake::Command(c),
                        None => return Ok(()),
                    },
                };
                match next {
                    Wake::Tick => {}
                    // Let a burst of CoreAudio callbacks settle into one read.
                    Wake::AudioChanged => tokio::time::sleep(Duration::from_millis(600)).await,
                    Wake::Playlists(result) => {
                        s.loading.remove(&result.0);
                        // A reload racing the browser sign-in must not erase
                        // the "waiting for approval" state.
                        if !(result.0 == MediaPlayer::Spotify && login.is_some()) {
                            apply_playlists(&mut s.state, result);
                        }
                    }
                    Wake::Login(r) => {
                        login = None;
                        match r {
                            Ok(Ok(())) => {
                                tracing::info!("signed in to Spotify");
                                apply_playlists(&mut s.state, (MediaPlayer::Spotify, PlaylistStatus::Unknown, vec![]));
                                s.reload_playlists(MediaPlayer::Spotify, &ctx, true);
                            }
                            Ok(Err(e)) => {
                                tracing::warn!(error = %e, "Spotify sign-in failed");
                                let failed = PlaylistStatus::Failed(e.to_string());
                                apply_playlists(&mut s.state, (MediaPlayer::Spotify, failed, vec![]));
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Spotify sign-in task failed");
                                let back = PlaylistStatus::NeedsSpotifyLogin;
                                apply_playlists(&mut s.state, (MediaPlayer::Spotify, back, vec![]));
                            }
                        }
                    }
                    Wake::Command(ProviderCommand::Media { player, action }) => {
                        s.control(player, &action).await;
                        tokio::time::sleep(SETTLE).await;
                    }
                    Wake::Command(ProviderCommand::MediaChanged(_)) => {}
                    Wake::Command(ProviderCommand::ReloadPlaylists) => {
                        for p in MediaPlayer::ALL {
                            s.reload_playlists(p, &ctx, false);
                        }
                    }
                    Wake::Command(ProviderCommand::SpotifyConnect) => {
                        if login.is_none() {
                            let cfg = ctx.config();
                            let http = s.http.clone();
                            let path = ctx.paths.spotify_token();
                            let port = cfg.media.spotify_redirect_port;
                            login = Some(tokio::spawn(async move {
                                spotify::login(&http, &path, port, notchforge_core::time::now_ms).await
                            }));
                            // Tell the UI the browser sign-in is open and waiting.
                            let waiting = PlaylistStatus::SigningIn;
                            apply_playlists(&mut s.state, (MediaPlayer::Spotify, waiting, vec![]));
                        }
                    }
                    Wake::Command(ProviderCommand::SpotifyCancel) => {
                        if let Some(h) = login.take() {
                            h.abort();
                            let back = PlaylistStatus::NeedsSpotifyLogin;
                            apply_playlists(&mut s.state, (MediaPlayer::Spotify, back, vec![]));
                        }
                    }
                    Wake::Command(ProviderCommand::SpotifyDisconnect) => {
                        let _ = std::fs::remove_file(ctx.paths.spotify_token());
                        let signed_out = PlaylistStatus::NeedsSpotifyLogin;
                        apply_playlists(&mut s.state, (MediaPlayer::Spotify, signed_out, vec![]));
                        s.playlists_loaded_ms.remove(&MediaPlayer::Spotify);
                    }
                    Wake::Command(ProviderCommand::SetOutputDevice(_)) => {}
                    Wake::Command(ProviderCommand::Usage { .. } | ProviderCommand::AgentHook(_)) => {}
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pl(player: MediaPlayer, id: &str) -> Playlist {
        Playlist {
            player,
            id: id.into(),
            name: id.into(),
            track_count: None,
            owner: None,
            artwork_url: None,
            artwork_path: None,
        }
    }

    #[test]
    fn playlists_merge_per_player_and_survive_failures() {
        let mut s = MediaState::default();
        apply_playlists(&mut s, (MediaPlayer::Music, PlaylistStatus::Loaded, vec![pl(MediaPlayer::Music, "a")]));
        apply_playlists(&mut s, (MediaPlayer::Spotify, PlaylistStatus::Loaded, vec![pl(MediaPlayer::Spotify, "b")]));
        assert_eq!(s.playlists.len(), 2);
        apply_playlists(&mut s, (MediaPlayer::Music, PlaylistStatus::Failed("x".into()), vec![]));
        assert_eq!(s.playlists_for(MediaPlayer::Music).count(), 1, "previous list kept");
        assert!(matches!(s.playlist_status(MediaPlayer::Music), PlaylistStatus::Failed(_)));
        apply_playlists(&mut s, (MediaPlayer::Spotify, PlaylistStatus::NeedsSpotifyLogin, vec![]));
        assert_eq!(s.playlists_for(MediaPlayer::Spotify).count(), 0, "sign-out clears the list");
    }

    #[test]
    fn cache_helpers() {
        assert_ne!(fnv("a"), fnv("b"));
        assert_eq!(fnv(""), 0xcbf2_9ce4_8422_2325);
        let dir = std::env::temp_dir().join(format!("nf-art-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        for i in 0..5 {
            std::fs::write(dir.join(format!("{i}.jpg")), b"x").expect("write");
        }
        prune_dir(&dir, 3);
        assert_eq!(std::fs::read_dir(&dir).expect("read").count(), 3);
        let _ = std::fs::remove_dir_all(dir);
    }
}
