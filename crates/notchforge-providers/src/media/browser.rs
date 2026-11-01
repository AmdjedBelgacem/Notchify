//! Audio playing in a web browser, shown as music.
//!
//! Browsers expose no player API, so the pieces come from two places:
//!
//! * **Is something playing?** CoreAudio's per-app activity ([`crate::audio`]),
//!   which needs no permission. A browser that is producing sound is "playing".
//! * **What is it?** The browser's tabs, read with AppleScript (Chromium
//!   browsers and Safari). The tab on a known media site wins, else the active
//!   tab. macOS asks for Automation per browser the first time; until it is
//!   allowed the card shows the browser's name instead of a title.
//!
//! Transport (play / pause / skip) is sent as system media keys by the macOS
//! layer, exactly like the keyboard's ⏯ key.

use notchforge_core::model::{AudioSource, MediaPlayer, NowPlaying, PlaybackState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Chrome, Arc, Brave, Edge, Vivaldi, Opera: `tabs of window` with `title` / `URL`.
    Chromium,
    Safari,
    /// No scripting dictionary (Firefox): only the app's name is known.
    Opaque,
}

#[derive(Debug, Clone, Copy)]
pub struct Browser {
    pub bundle_id: &'static str,
    pub name: &'static str,
    pub family: Family,
}

pub const BROWSERS: &[Browser] = &[
    Browser { bundle_id: "com.google.Chrome", name: "Chrome", family: Family::Chromium },
    Browser { bundle_id: "com.google.Chrome.canary", name: "Chrome Canary", family: Family::Chromium },
    Browser { bundle_id: "company.thebrowser.Browser", name: "Arc", family: Family::Chromium },
    Browser { bundle_id: "company.thebrowser.dia", name: "Dia", family: Family::Chromium },
    Browser { bundle_id: "com.brave.Browser", name: "Brave", family: Family::Chromium },
    Browser { bundle_id: "com.microsoft.edgemac", name: "Edge", family: Family::Chromium },
    Browser { bundle_id: "com.vivaldi.Vivaldi", name: "Vivaldi", family: Family::Chromium },
    Browser { bundle_id: "com.operasoftware.Opera", name: "Opera", family: Family::Chromium },
    Browser { bundle_id: "com.apple.Safari", name: "Safari", family: Family::Safari },
    Browser { bundle_id: "com.apple.SafariTechnologyPreview", name: "Safari Tech Preview", family: Family::Safari },
    Browser { bundle_id: "org.mozilla.firefox", name: "Firefox", family: Family::Opaque },
    Browser { bundle_id: "app.zen-browser.zen", name: "Zen", family: Family::Opaque },
];

/// Sites that are, in practice, music or video players: (host suffix, service).
const SERVICES: &[(&str, &str)] = &[
    ("music.youtube.com", "YouTube Music"),
    ("youtube.com", "YouTube"),
    ("youtu.be", "YouTube"),
    ("soundcloud.com", "SoundCloud"),
    ("open.spotify.com", "Spotify Web"),
    ("music.apple.com", "Apple Music Web"),
    ("deezer.com", "Deezer"),
    ("tidal.com", "Tidal"),
    ("bandcamp.com", "Bandcamp"),
    ("mixcloud.com", "Mixcloud"),
    ("twitch.tv", "Twitch"),
    ("netflix.com", "Netflix"),
    ("primevideo.com", "Prime Video"),
    ("disneyplus.com", "Disney+"),
    ("vimeo.com", "Vimeo"),
    ("pandora.com", "Pandora"),
    ("audiomack.com", "Audiomack"),
    ("podcasts.google.com", "Google Podcasts"),
    ("overcast.fm", "Overcast"),
];

/// The browser (if any) among the apps currently producing audio.
pub fn find(sources: &[AudioSource]) -> Option<(&'static Browser, &AudioSource)> {
    sources.iter().find_map(|s| {
        let id = s.bundle_id.as_deref()?;
        BROWSERS.iter().find(|b| b.bundle_id.eq_ignore_ascii_case(id)).map(|b| (b, s))
    })
}

/// `title␟url` per tab, the active tab first.
pub fn tabs_script(b: &Browser) -> Option<String> {
    let id = b.bundle_id;
    match b.family {
        Family::Chromium => Some(format!(
            r#"on run
	set d to character id 31
	set out to ""
	tell application id "{id}"
		try
			set a to active tab of front window
			set out to (title of a) & d & (URL of a) & linefeed
		end try
		set n to 0
		repeat with w in windows
			repeat with t in tabs of w
				set n to n + 1
				if n > 80 then exit repeat
				set out to out & (title of t) & d & (URL of t) & linefeed
			end repeat
		end repeat
	end tell
	return out
end run"#
        )),
        Family::Safari => Some(format!(
            r#"on run
	set d to character id 31
	set out to ""
	tell application id "{id}"
		try
			set a to current tab of front window
			set out to (name of a) & d & (URL of a) & linefeed
		end try
		set n to 0
		repeat with w in windows
			repeat with t in tabs of w
				set n to n + 1
				if n > 80 then exit repeat
				set out to out & (name of t) & d & (URL of t) & linefeed
			end repeat
		end repeat
	end tell
	return out
end run"#
        )),
        Family::Opaque => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub title: String,
    pub url: String,
}

/// Parse [`tabs_script`] output; the first tab is the active one.
pub fn parse_tabs(out: &str) -> Vec<Tab> {
    out.lines()
        .filter_map(|l| {
            let (t, u) = l.split_once('\u{1f}')?;
            Some(Tab { title: t.trim().to_owned(), url: u.trim().to_owned() })
        })
        .filter(|t| !t.title.is_empty() || !t.url.is_empty())
        .collect()
}

fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or("").trim_start_matches("www.")
}

/// The media service a URL belongs to.
pub fn service(url: &str) -> Option<&'static str> {
    let h = host(url);
    SERVICES.iter().find(|(s, _)| h == *s || h.ends_with(&format!(".{s}"))).map(|(_, n)| *n)
}

/// Choose the tab that is most likely the one making sound: the active tab if
/// it is on a media site, else the first media tab, else the active tab.
pub fn pick(tabs: &[Tab]) -> Option<&Tab> {
    let active = tabs.first()?;
    if service(&active.url).is_some() {
        return Some(active);
    }
    tabs.iter().skip(1).find(|t| service(&t.url).is_some()).or(Some(active))
}

/// Strip the site's own suffix and a leading notification count:
/// `"(3) Daft Punk - One More Time - YouTube"` → `"Daft Punk - One More Time"`.
pub fn clean_title(title: &str, service_name: Option<&str>) -> String {
    let mut t = title.trim();
    if t.starts_with('(') {
        if let Some((count, rest)) = t.split_once(')') {
            if count[1..].chars().all(|c| c.is_ascii_digit()) {
                t = rest.trim_start();
            }
        }
    }
    let mut s = t.to_owned();
    for sep in [" - ", " | ", " – ", " · "] {
        if let Some((head, tail)) = s.rsplit_once(sep) {
            let tail_l = tail.to_lowercase();
            let site = service_name.map(str::to_lowercase).unwrap_or_default();
            let known = [
                "youtube",
                "youtube music",
                "soundcloud",
                "spotify",
                "deezer",
                "tidal",
                "bandcamp",
                "twitch",
                "netflix",
                "vimeo",
                "mixcloud",
            ]
            .iter()
            .any(|k| tail_l == *k || tail_l.starts_with(k));
            if known || (!site.is_empty() && tail_l.starts_with(&site)) {
                s = head.trim().to_owned();
                break;
            }
        }
    }
    s
}

/// The YouTube video id of a watch URL, for its thumbnail.
pub fn youtube_id(url: &str) -> Option<String> {
    let h = host(url);
    if h == "youtu.be" {
        let id = url.split_once("youtu.be/")?.1.split(['?', '&', '#', '/']).next()?;
        return (id.len() == 11).then(|| id.to_owned());
    }
    if !(h == "youtube.com" || h.ends_with(".youtube.com")) {
        return None;
    }
    let q = url.split_once('?')?.1;
    q.split('&')
        .find_map(|kv| kv.strip_prefix("v="))
        .map(|v| v.split('#').next().unwrap_or(v))
        .filter(|v| v.len() == 11)
        .map(str::to_owned)
}

/// Build the now-playing entry for a browser that is making sound.
pub fn now_playing(b: &Browser, tab: Option<&Tab>, state: PlaybackState, now_ms: i64) -> NowPlaying {
    let (title, artist, track_id) = match tab {
        Some(t) => {
            let svc = service(&t.url);
            let title = clean_title(&t.title, svc);
            let title = if title.is_empty() { b.name.to_owned() } else { title };
            (title, svc.map_or_else(|| host(&t.url).to_owned(), str::to_owned), Some(t.url.clone()))
        }
        None => (b.name.to_owned(), "Playing audio".to_owned(), None),
    };
    NowPlaying {
        player: MediaPlayer::Browser,
        state,
        title,
        artist,
        album: b.name.to_owned(),
        duration: None,
        position: None,
        sampled_at_ms: now_ms,
        track_id,
        artwork_path: None,
        shuffle: None,
        repeat: None,
        volume: None,
        source_bundle: Some(b.bundle_id.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(t: &str, u: &str) -> Tab {
        Tab { title: t.into(), url: u.into() }
    }

    #[test]
    fn finds_browsers_among_audio_sources() {
        let src = |id: &str| AudioSource {
            pid: 1,
            name: "x".into(),
            bundle_id: Some(id.into()),
            app_path: None,
            device_uids: vec![],
        };
        let list = [src("com.spotify.client"), src("com.google.Chrome")];
        assert_eq!(find(&list).map(|(b, _)| b.name), Some("Chrome"));
        assert!(find(&[src("com.spotify.client")]).is_none());
    }

    #[test]
    fn picks_the_media_tab_and_cleans_titles() {
        let tabs = parse_tabs(
            "Inbox (4)\u{1f}https://mail.example.com/\n\
             (2) Daft Punk - One More Time - YouTube\u{1f}https://www.youtube.com/watch?v=FGBhQbmPwH8&t=3\n\
             Docs\u{1f}https://docs.example.com\n",
        );
        let t = pick(&tabs).expect("tab");
        assert_eq!(service(&t.url), Some("YouTube"));
        assert_eq!(clean_title(&t.title, service(&t.url)), "Daft Punk - One More Time");
        assert_eq!(youtube_id(&t.url).as_deref(), Some("FGBhQbmPwH8"));
        // With no media tab, the active tab is used.
        assert_eq!(pick(&tabs[..1]).map(|t| t.title.as_str()), Some("Inbox (4)"));
    }

    #[test]
    fn titles_without_a_site_suffix_are_kept() {
        assert_eq!(clean_title("Plain - Title", None), "Plain - Title");
        assert_eq!(clean_title("Mix | SoundCloud", Some("SoundCloud")), "Mix");
        assert_eq!(youtube_id("https://youtu.be/FGBhQbmPwH8?t=1").as_deref(), Some("FGBhQbmPwH8"));
        assert!(youtube_id("https://example.com/watch?v=FGBhQbmPwH8").is_none());
    }

    #[test]
    fn entry_is_a_browser_track() {
        let t = tab("Song - SoundCloud", "https://soundcloud.com/a/b");
        let np = now_playing(&BROWSERS[0], Some(&t), PlaybackState::Playing, 5);
        assert_eq!((np.player, np.title.as_str(), np.artist.as_str()), (MediaPlayer::Browser, "Song", "SoundCloud"));
        assert_eq!(np.album, "Chrome");
        let bare = now_playing(&BROWSERS[0], None, PlaybackState::Paused, 5);
        assert_eq!(bare.title, "Chrome");
    }
}
