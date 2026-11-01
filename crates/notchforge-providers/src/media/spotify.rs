//! Spotify Web API, used only to list the user's playlists (Spotify's
//! AppleScript dictionary has no playlist access). Playback itself always goes
//! through AppleScript, so it works for Free and Premium accounts alike.
//!
//! Sign-in is OAuth 2.0 *Authorization Code with PKCE*: no client secret is
//! stored. The user registers their own app at developer.spotify.com, adds the
//! redirect URI `http://127.0.0.1:<port>/callback`, and pastes the client ID in
//! Settings. NotchForge opens the browser, receives the redirect on a one-shot
//! loopback listener, and stores only the refresh token (file mode 0600).

use base64::Engine as _;
use notchforge_core::model::{MediaPlayer, Playlist};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
pub const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
pub const API_BASE: &str = "https://api.spotify.com/v1";
pub const SCOPES: &str = "playlist-read-private playlist-read-collaborative";
/// How long the loopback listener waits for the browser redirect.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, thiserror::Error)]
pub enum SpotifyError {
    #[error("no Spotify client ID configured")]
    NoClientId,
    #[error("not signed in to Spotify")]
    NotSignedIn,
    #[error("sign-in was cancelled or timed out")]
    LoginAborted,
    #[error("Spotify rejected the request: {0}")]
    Rejected(String),
    #[error("network: {0}")]
    Http(#[from] reqwest::Error),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
}

// ─────────────────────────────────────────────────────────────────────────────
// PKCE
// ─────────────────────────────────────────────────────────────────────────────

pub fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 32 random bytes from the OS.
pub fn random_bytes() -> std::io::Result<[u8; 32]> {
    let mut buf = [0u8; 32];
    let mut f = std::fs::File::open("/dev/urandom")?;
    std::io::Read::read_exact(&mut f, &mut buf)?;
    Ok(buf)
}

/// (verifier, S256 challenge).
pub fn pkce_pair(seed: &[u8]) -> (String, String) {
    let verifier = b64url(seed);
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

pub fn authorize_url(client_id: &str, redirect_uri: &str, challenge: &str, state: &str) -> String {
    let mut url = reqwest::Url::parse(AUTHORIZE_URL).expect("static URL");
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", challenge)
        .append_pair("scope", SCOPES)
        .append_pair("state", state);
    url.into()
}

/// Extract `code` (checking `state`) from the redirect's request line, e.g.
/// `GET /callback?code=…&state=… HTTP/1.1`.
pub fn parse_callback(request_line: &str, expected_state: &str) -> Result<String, String> {
    let target = request_line.split_whitespace().nth(1).ok_or("malformed request")?;
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}")).map_err(|e| e.to_string())?;
    if url.path() != "/callback" {
        return Err("unexpected path".into());
    }
    let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    if let Some(err) = q.get("error") {
        return Err(format!("Spotify said: {err}"));
    }
    if q.get("state").map(String::as_str) != Some(expected_state) {
        return Err("state mismatch".into());
    }
    q.get("code").cloned().ok_or_else(|| "no code in redirect".into())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tokens
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix ms.
    pub expires_at_ms: i64,
    /// Client ID the token was issued to (a different ID invalidates it).
    pub client_id: String,
}

impl Token {
    pub fn is_fresh(&self, now_ms: i64) -> bool {
        self.expires_at_ms - 60_000 > now_ms
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
    refresh_token: Option<String>,
}

pub fn load_token(path: &Path) -> Option<Token> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub fn save_token(path: &Path, t: &Token) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(t).map_err(std::io::Error::other)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(tmp, path)
}

async fn token_request(http: &reqwest::Client, form: &[(&str, &str)]) -> Result<TokenResponse, SpotifyError> {
    let resp = http.post(TOKEN_URL).form(form).send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(SpotifyError::Rejected(format!("{status}: {}", crate::util::truncate(&body, 200))));
    }
    Ok(resp.json().await?)
}

/// A valid access token, refreshing (and persisting) it when needed.
pub async fn access_token(
    http: &reqwest::Client,
    path: &Path,
    client_id: &str,
    now_ms: i64,
) -> Result<Token, SpotifyError> {
    if client_id.is_empty() {
        return Err(SpotifyError::NoClientId);
    }
    let t = load_token(path).filter(|t| t.client_id == client_id).ok_or(SpotifyError::NotSignedIn)?;
    if t.is_fresh(now_ms) {
        return Ok(t);
    }
    let r = token_request(
        http,
        &[("grant_type", "refresh_token"), ("refresh_token", &t.refresh_token), ("client_id", client_id)],
    )
    .await?;
    let t = Token {
        access_token: r.access_token,
        refresh_token: r.refresh_token.unwrap_or(t.refresh_token),
        expires_at_ms: now_ms + r.expires_in * 1000,
        client_id: client_id.to_owned(),
    };
    save_token(path, &t)?;
    Ok(t)
}

/// Full sign-in: opens the browser, waits for the redirect, stores the token.
pub async fn login(
    http: &reqwest::Client,
    path: &Path,
    client_id: &str,
    port: u16,
    now_ms: impl Fn() -> i64,
) -> Result<(), SpotifyError> {
    if client_id.is_empty() {
        return Err(SpotifyError::NoClientId);
    }
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let (verifier, challenge) = pkce_pair(&random_bytes()?);
    let state = b64url(&random_bytes()?[..16]);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let url = authorize_url(client_id, &redirect, &challenge, &state);
    open_browser(&url).await;

    let code = tokio::time::timeout(LOGIN_TIMEOUT, async {
        loop {
            let (mut sock, _) = listener.accept().await?;
            let mut buf = vec![0u8; 8192];
            let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf)).await.unwrap_or(Ok(0))?;
            let req = String::from_utf8_lossy(&buf[..n]);
            let line = req.lines().next().unwrap_or("");
            let result = parse_callback(line, &state);
            let (status, msg) = match &result {
                Ok(_) => ("200 OK", "Notchisland is connected to Spotify. You can close this tab."),
                Err(_) if !line.contains("/callback") => ("404 Not Found", "Not found."),
                Err(_) => ("400 Bad Request", "Spotify sign-in failed. Close this tab and try again from Notchisland."),
            };
            let body = format!(
                "<!doctype html><meta charset=utf-8><title>Notchisland</title>\
                 <body style=\"font:16px -apple-system;margin:4em;text-align:center\">{msg}</body>"
            );
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            match result {
                Ok(code) => return Ok::<_, std::io::Error>(Some(code)),
                Err(e) if line.contains("/callback") => {
                    tracing::warn!(error = %e, "Spotify sign-in redirect rejected");
                    return Ok(None);
                }
                Err(_) => continue, // favicon etc.
            }
        }
    })
    .await
    .map_err(|_| SpotifyError::LoginAborted)??
    .ok_or(SpotifyError::LoginAborted)?;

    let r = token_request(
        http,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect),
            ("client_id", client_id),
            ("code_verifier", &verifier),
        ],
    )
    .await?;
    let t = Token {
        access_token: r.access_token,
        refresh_token: r.refresh_token.ok_or_else(|| SpotifyError::Rejected("no refresh token".into()))?,
        expires_at_ms: now_ms() + r.expires_in * 1000,
        client_id: client_id.to_owned(),
    };
    save_token(path, &t)?;
    Ok(())
}

async fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let program = "/usr/bin/open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    if let Err(e) = tokio::process::Command::new(program).arg(url).status().await {
        tracing::warn!(error = %e, url, "could not open the browser; open the URL manually");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Playlists
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Page {
    items: Vec<Option<ApiPlaylist>>,
    next: Option<String>,
}

#[derive(Deserialize)]
struct Count {
    total: Option<u32>,
}

#[derive(Deserialize)]
struct Image {
    url: String,
    width: Option<u32>,
}

#[derive(Deserialize)]
struct Owner {
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct ApiPlaylist {
    name: String,
    uri: String,
    /// Track count lives in `tracks` (older API) or `items` (newer API).
    tracks: Option<Count>,
    items: Option<Count>,
    #[serde(default)]
    images: Option<Vec<Image>>,
    owner: Option<Owner>,
}

/// Parse one page of `GET /me/playlists`; returns playlists and the next URL.
pub fn parse_playlists_page(json: &str) -> Result<(Vec<Playlist>, Option<String>), serde_json::Error> {
    let page: Page = serde_json::from_str(json)?;
    let lists = page
        .items
        .into_iter()
        .flatten()
        .map(|p| {
            // Prefer the ~300 px image: big enough for the UI, small to download.
            let artwork_url = p
                .images
                .unwrap_or_default()
                .into_iter()
                .min_by_key(|i| i.width.unwrap_or(640).abs_diff(300))
                .map(|i| i.url);
            Playlist {
                player: MediaPlayer::Spotify,
                id: p.uri,
                name: p.name,
                track_count: p.tracks.and_then(|c| c.total).or(p.items.and_then(|c| c.total)),
                owner: p.owner.and_then(|o| o.display_name),
                artwork_url,
                artwork_path: None,
            }
        })
        .collect();
    Ok((lists, page.next))
}

pub async fn playlists(http: &reqwest::Client, token: &Token, max: usize) -> Result<Vec<Playlist>, SpotifyError> {
    let mut url = Some(format!("{API_BASE}/me/playlists?limit=50"));
    let mut out = Vec::new();
    while let Some(u) = url.take() {
        let resp = http.get(&u).bearer_auth(&token.access_token).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(SpotifyError::Rejected(format!("{status} listing playlists")));
        }
        let text = resp.text().await?;
        let (mut page, next) =
            parse_playlists_page(&text).map_err(|e| SpotifyError::Rejected(format!("unexpected response: {e}")))?;
        out.append(&mut page);
        if out.len() >= max {
            out.truncate(max);
            break;
        }
        url = next;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc7636_example() {
        // RFC 7636 appendix B.
        let seed = [
            116, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212, 37, 77, 105, 214, 191,
            240, 91, 88, 5, 88, 83, 132, 141, 121,
        ];
        let (verifier, challenge) = pkce_pair(&seed);
        assert_eq!(verifier, "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn authorize_url_is_encoded() {
        let u = authorize_url("abc", "http://127.0.0.1:47816/callback", "CH", "ST");
        assert!(u.starts_with("https://accounts.spotify.com/authorize?client_id=abc&response_type=code"));
        assert!(u.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A47816%2Fcallback"));
        assert!(u.contains("scope=playlist-read-private+playlist-read-collaborative"));
    }

    #[test]
    fn callback_parsing() {
        assert_eq!(parse_callback("GET /callback?code=XYZ&state=s1 HTTP/1.1", "s1"), Ok("XYZ".into()));
        assert!(parse_callback("GET /callback?code=XYZ&state=evil HTTP/1.1", "s1").is_err());
        assert!(parse_callback("GET /callback?error=access_denied&state=s1 HTTP/1.1", "s1").is_err());
        assert!(parse_callback("GET /favicon.ico HTTP/1.1", "s1").is_err());
    }

    #[test]
    fn playlist_pages_parse_old_and_new_shapes() {
        let json = r#"{
          "items": [
            {"name": "Chill", "uri": "spotify:playlist:1", "tracks": {"total": 12},
             "images": [{"url": "https://i/640", "width": 640}, {"url": "https://i/300", "width": 300}],
             "owner": {"display_name": "me"}},
            {"name": "New shape", "uri": "spotify:playlist:2", "items": {"total": 3}, "images": null, "owner": null},
            null
          ],
          "next": "https://api.spotify.com/v1/me/playlists?offset=50"
        }"#;
        let (lists, next) = parse_playlists_page(json).expect("parse");
        assert_eq!(lists.len(), 2);
        assert_eq!(lists[0].track_count, Some(12));
        assert_eq!(lists[0].artwork_url.as_deref(), Some("https://i/300"));
        assert_eq!(lists[1].track_count, Some(3));
        assert!(next.is_some());
    }

    #[test]
    fn token_round_trip_and_freshness() {
        let dir = std::env::temp_dir().join(format!("nf-spotify-{}", std::process::id()));
        let path = dir.join("t.json");
        let t = Token {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at_ms: 100_000,
            client_id: "c".into(),
        };
        save_token(&path, &t).expect("save");
        assert_eq!(load_token(&path), Some(t.clone()));
        assert!(t.is_fresh(0));
        assert!(!t.is_fresh(50_000));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
