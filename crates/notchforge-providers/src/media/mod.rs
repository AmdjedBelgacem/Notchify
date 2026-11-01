//! Media provider: Spotify and Apple Music — now playing, cover art, transport
//! controls and playlists.
//!
//! * **Now playing** comes from each player's AppleScript dictionary
//!   ([`script`]), read only while the player is running (checked with a
//!   process lookup, so we never launch a player by asking about it). The macOS
//!   layer forwards the players' own change announcements
//!   (`com.spotify.client.PlaybackStateChanged`, `com.apple.Music.playerInfo`)
//!   as [`ProviderCommand::MediaChanged`], so the poll can stay slow.
//! * **Cover art** is cached under `artwork/`: Spotify's artwork URL is
//!   downloaded; Music's embedded artwork is exported by a script.
//! * **Playlists**: Music via AppleScript; Spotify via the Web API
//!   ([`spotify`]) once the user has signed in. Loading runs in a background
//!   task so controls stay responsive.

pub mod script;
pub mod spotify;

use notchforge_core::config::Paths;
use notchforge_core::model::{MediaAction, MediaPlayer, MediaState, NowPlaying, Playlist, PlaylistStatus};
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
pub fn fnv(s: &str) -> u64 {
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

async fn download(http: &reqwest::Client, url: &str, dest: &Path) -> Option<PathBuf> {
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
    Command(ProviderCommand),
}

/// Load one player's playlists (runs in a background task).
async fn load_playlists(
    player: MediaPlayer,
    http: reqwest::Client,
    paths: std::sync::Arc<Paths>,
    client_id: String,
    max: usize,
) -> PlaylistResult {
    match player {
        MediaPlayer::Music => match script::run(script::MUSIC_PLAYLISTS_SCRIPT, &[&max.to_string()]).await {
            Ok(out) => (player, PlaylistStatus::Loaded, script::parse_music_playlists(&out)),
            Err(script::ScriptError::NotPermitted) => (player, PlaylistStatus::NeedsAutomation, vec![]),
            Err(e) => (player, PlaylistStatus::Failed(e.to_string()), vec![]),
        },
        MediaPlayer::Spotify => {
            let now = notchforge_core::time::now_ms();
            match spotify::access_token(&http, &paths.spotify_token(), &client_id, now).await {
                Ok(token) => match spotify::playlists(&http, &token, max).await {
                    Ok(mut lists) => {
                        let dir = paths.artwork_dir();
                        let _ = tokio::fs::create_dir_all(&dir).await;
                        fetch_playlist_art(&http, &dir, &mut lists).await;
                        (player, PlaylistStatus::Loaded, lists)
                    }
                    Err(e) => (player, PlaylistStatus::Failed(e.to_string()), vec![]),
                },
                Err(spotify::SpotifyError::NoClientId | spotify::SpotifyError::NotSignedIn) => {
                    (player, PlaylistStatus::NeedsSpotifyLogin, vec![])
                }
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
}

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

    /// Read every running player. Returns players that just started running.
    async fn read_players(&mut self) -> Vec<MediaPlayer> {
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
        let started = running.iter().copied().filter(|p| !self.state.running.contains(p)).collect();
        self.state.players = players;
        self.state.running = running;
        self.state.automation_denied = denied;
        started
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
        let fut = load_playlists(
            player,
            self.http.clone(),
            self.paths.clone(),
            cfg.media.spotify_client_id.clone(),
            cfg.media.max_playlists,
        );
        tokio::spawn(async move {
            let _ = tx.send(fut.await).await;
        });
    }

    async fn control(&mut self, player: MediaPlayer, action: &MediaAction) {
        // Only "play this playlist" may launch a player; other controls are
        // meaningless when it is not running.
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
            };
            let mut login: Option<tokio::task::JoinHandle<Result<(), spotify::SpotifyError>>> = None;
            let mut last_emitted: Option<MediaState> = None;
            let mut first = true;
            ctx.set_status(ProviderStatus::Running).await;

            loop {
                let started = s.read_players().await;
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

                let poll = if s.state.running.is_empty() {
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
                let next = tokio::select! {
                    _ = tokio::time::sleep(poll) => Wake::Tick,
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
                            let (id, port) = (cfg.media.spotify_client_id.clone(), cfg.media.spotify_redirect_port);
                            login = Some(tokio::spawn(async move {
                                spotify::login(&http, &path, &id, port, notchforge_core::time::now_ms).await
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
