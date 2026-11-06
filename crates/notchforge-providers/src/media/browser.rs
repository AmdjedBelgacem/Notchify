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

/// The media service a URL belongs to, via the core site registry.
pub fn service(url: &str) -> Option<&'static str> {
    notchforge_core::sites::site_for_host(host(url)).filter(|s| s.media).map(|s| s.name)
}

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

/// The host of a URL, without `www.` (empty for a title-only tab).
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or("").trim_start_matches("www.")
}

/// True for a URL that points at a specific video (a watch page), as opposed
/// to a media site's home or feed — feed autoplay makes *every* media site
/// look like it's the one playing.
pub fn is_watch_url(url: &str) -> bool {
    youtube_id(url).is_some()
        || url.contains("/watch")
        || url.contains("/shorts/")
        || url.contains("/videos/")
        || url.contains("/reel/")
        || url.contains("/tv/")
}

/// Choose the tab that is most likely the one making sound: a watch page
/// (a specific video) beats a mere media-site tab — feed autoplay makes home
/// pages look audible — then the active media tab, else the active tab.
pub fn pick(tabs: &[Tab]) -> Option<&Tab> {
    let active = tabs.first()?;
    if is_watch_url(&active.url) {
        return Some(active);
    }
    if let Some(t) = tabs.iter().skip(1).find(|t| is_watch_url(&t.url)) {
        return Some(t);
    }
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

/// The video query in a tab title when the page is YouTube but we have no
/// URL (browsers whose tabs cannot be read give the window title only):
/// `"(2) Daft Punk - One More Time - YouTube"` → `"Daft Punk - One More Time"`.
/// `None` for a bare "YouTube" (a feed, no specific video) or any other site.
pub fn youtube_title_query(title: &str) -> Option<String> {
    let lowered = title.to_lowercase();
    if !lowered.contains("youtube") {
        return None;
    }
    let t = clean_title(title, Some("YouTube"));
    let t = t.trim();
    let bare = t.is_empty() || t.eq_ignore_ascii_case("youtube") || t.eq_ignore_ascii_case("youtube music");
    (!bare).then(|| t.to_owned())
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

/// Build the now-playing entry for a browser that is making sound. `tab` may
/// be title-only (empty URL) when it came from the Accessibility fallback.
pub fn now_playing(b: &Browser, tab: Option<&Tab>, state: PlaybackState, now_ms: i64) -> NowPlaying {
    let (title, artist, track_id) = match tab {
        Some(t) => {
            let svc = service(&t.url);
            let mut title = clean_title(&t.title, svc);
            // Browsers append their own name to window titles ("GitHub - Dia"),
            // which is all the Accessibility fallback sees.
            for sep in [" - ", " – ", " | ", " · "] {
                if let Some((head, tail)) = title.rsplit_once(sep) {
                    if tail.eq_ignore_ascii_case(b.name) {
                        title = head.trim().to_owned();
                        break;
                    }
                }
            }
            // "YouTube" titled "YouTube" would read twice on the card.
            let title = match svc {
                Some(site) if title.eq_ignore_ascii_case(site) => format!("Playing from {site}"),
                _ => title,
            };
            let title = if title.is_empty() { b.name.to_owned() } else { title };
            let artist = match svc {
                Some(name) => name.to_owned(),
                None => {
                    let h = host(&t.url);
                    if h.is_empty() {
                        // Title-only: name the site if the title gives it away.
                        notchforge_core::sites::site_from_text(&t.title)
                            .map_or_else(|| "Playing audio".to_owned(), |s| s.name.to_owned())
                    } else {
                        h.to_owned()
                    }
                }
            };
            (title, artist, (!t.url.is_empty()).then(|| t.url.clone()))
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
    fn youtube_titles_give_a_search_query() {
        assert_eq!(
            youtube_title_query("(2) Daft Punk - One More Time - YouTube").as_deref(),
            Some("Daft Punk - One More Time")
        );
        assert_eq!(youtube_title_query("Some Video - YouTube").as_deref(), Some("Some Video"));
        // A bare feed title names no video.
        assert_eq!(youtube_title_query("YouTube"), None);
        assert_eq!(youtube_title_query("(12) YouTube"), None);
        assert_eq!(youtube_title_query("YouTube Music"), None);
        // Any other site is not YouTube's business.
        assert_eq!(youtube_title_query("Inbox (4) - Gmail"), None);
        assert_eq!(youtube_title_query("GitHub - Dia"), None);
    }

    #[test]
    fn a_watch_page_beats_a_feed_homepage() {
        // Facebook's feed is the active tab (autoplaying video), but the
        // YouTube watch tab is the one the user put a video on.
        let tabs = parse_tabs(
            "(1) Facebook\u{1f}https://www.facebook.com/\n             Daft Punk - One More Time - YouTube\u{1f}https://www.youtube.com/watch?v=FGBhQbmPwH8\n",
        );
        let t = pick(&tabs).expect("tab");
        assert_eq!(service(&t.url), Some("YouTube"));
        assert_eq!(youtube_id(&t.url).as_deref(), Some("FGBhQbmPwH8"));
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
