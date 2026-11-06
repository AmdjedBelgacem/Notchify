//! Web platforms: sites that play media in a browser tab, or send
//! notifications through a browser (web push).
//!
//! One static table feeds three users:
//!
//! * **Browser audio** (`notchforge-providers` media/browser): a tab's URL is
//!   matched against `hosts` to name the service it belongs to, and the site's
//!   favicon becomes the card's artwork when the site has no video thumbnail.
//! * **Notification attribution** (`apps::AppRef::resolve`): a web push that
//!   arrives through a browser is attributed to the site named in its text
//!   (macOS puts the origin there), so the notch shows "YouTube" — not
//!   "Chrome" — with its own group, filter rule and logo.
//! * **Icons**: `key` names the cached logo file; `accent`/`symbol` draw the
//!   placeholder tile until (or without) the favicon.
//!
//! The table is data-driven on purpose: supporting another platform is one
//! entry here, nothing else.

/// One known web platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebSite {
    /// Stable identifier: rule key, group key and icon file name.
    pub key: &'static str,
    /// Display name ("YouTube").
    pub name: &'static str,
    /// Host suffixes a tab URL or push origin matches.
    pub hosts: &'static [&'static str],
    /// Extra text hints matched (case-insensitively) inside a notification's
    /// title/subtitle/body — the site's own name where it is distinctive.
    pub hints: &'static [&'static str],
    /// Brand colour for the placeholder tile.
    pub accent: (u8, u8, u8),
    /// SF Symbol drawn on the placeholder tile.
    pub symbol: &'static str,
    /// A site that is, in practice, a music or video player.
    pub media: bool,
}

/// All known platforms. Media sites first (browser tab matching scans in
/// order, so more specific hosts like `music.youtube.com` must come first).
pub const SITES: &[WebSite] = &[
    WebSite {
        key: "youtube-music",
        name: "YouTube Music",
        hosts: &["music.youtube.com"],
        hints: &["music.youtube.com", "youtube music"],
        accent: (255, 0, 0),
        symbol: "play.rectangle.fill",
        media: true,
    },
    WebSite {
        key: "youtube",
        name: "YouTube",
        hosts: &["youtube.com", "youtu.be"],
        hints: &["youtube.com", "youtu.be", "youtube", " - youtube"],
        accent: (255, 0, 0),
        symbol: "play.rectangle.fill",
        media: true,
    },
    WebSite {
        key: "soundcloud",
        name: "SoundCloud",
        hosts: &["soundcloud.com"],
        hints: &["soundcloud.com", "soundcloud"],
        accent: (255, 85, 0),
        symbol: "waveform",
        media: true,
    },
    WebSite {
        key: "spotify-web",
        name: "Spotify Web",
        hosts: &["open.spotify.com"],
        hints: &[],
        accent: (30, 215, 96),
        symbol: "music.note",
        media: true,
    },
    WebSite {
        key: "apple-music-web",
        name: "Apple Music Web",
        hosts: &["music.apple.com"],
        hints: &[],
        accent: (250, 45, 68),
        symbol: "music.note",
        media: true,
    },
    WebSite {
        key: "deezer",
        name: "Deezer",
        hosts: &["deezer.com"],
        hints: &[],
        accent: (162, 39, 219),
        symbol: "music.note",
        media: true,
    },
    WebSite {
        key: "tidal",
        name: "Tidal",
        hosts: &["tidal.com"],
        hints: &[],
        accent: (0, 0, 0),
        symbol: "music.note",
        media: true,
    },
    WebSite {
        key: "bandcamp",
        name: "Bandcamp",
        hosts: &["bandcamp.com"],
        hints: &[],
        accent: (98, 165, 178),
        symbol: "music.note",
        media: true,
    },
    WebSite {
        key: "mixcloud",
        name: "Mixcloud",
        hosts: &["mixcloud.com"],
        hints: &[],
        accent: (85, 255, 255),
        symbol: "waveform",
        media: true,
    },
    WebSite {
        key: "twitch",
        name: "Twitch",
        hosts: &["twitch.tv"],
        hints: &["twitch.tv"],
        accent: (145, 70, 255),
        symbol: "gamecontroller.fill",
        media: true,
    },
    WebSite {
        key: "netflix",
        name: "Netflix",
        hosts: &["netflix.com"],
        hints: &["netflix"],
        accent: (229, 9, 20),
        symbol: "play.rectangle.fill",
        media: true,
    },
    WebSite {
        key: "prime-video",
        name: "Prime Video",
        hosts: &["primevideo.com"],
        hints: &["prime video"],
        accent: (0, 168, 255),
        symbol: "play.rectangle.fill",
        media: true,
    },
    WebSite {
        key: "disney-plus",
        name: "Disney+",
        hosts: &["disneyplus.com"],
        hints: &["disney+"],
        accent: (26, 36, 69),
        symbol: "play.rectangle.fill",
        media: true,
    },
    WebSite {
        key: "vimeo",
        name: "Vimeo",
        hosts: &["vimeo.com"],
        hints: &["vimeo"],
        accent: (26, 183, 234),
        symbol: "play.rectangle.fill",
        media: true,
    },
    WebSite {
        key: "pandora",
        name: "Pandora",
        hosts: &["pandora.com"],
        hints: &[],
        accent: (0, 71, 133),
        symbol: "music.note",
        media: true,
    },
    WebSite {
        key: "audiomack",
        name: "Audiomack",
        hosts: &["audiomack.com"],
        hints: &[],
        accent: (255, 164, 38),
        symbol: "music.note",
        media: true,
    },
    WebSite {
        key: "google-podcasts",
        name: "Google Podcasts",
        hosts: &["podcasts.google.com"],
        hints: &[],
        accent: (66, 133, 244),
        symbol: "mic.fill",
        media: true,
    },
    WebSite {
        key: "overcast",
        name: "Overcast",
        hosts: &["overcast.fm"],
        hints: &[],
        accent: (252, 105, 34),
        symbol: "mic.fill",
        media: true,
    },
    WebSite {
        key: "instagram",
        name: "Instagram",
        hosts: &["instagram.com"],
        hints: &["instagram.com", "instagram"],
        accent: (225, 48, 108),
        symbol: "camera.fill",
        media: true,
    },
    WebSite {
        key: "facebook",
        name: "Facebook",
        hosts: &["facebook.com"],
        hints: &["facebook.com"],
        accent: (24, 119, 242),
        symbol: "person.2.fill",
        media: true,
    },
    WebSite {
        key: "reddit",
        name: "Reddit",
        hosts: &["reddit.com", "redd.it"],
        hints: &["reddit.com", "redd.it", "reddit"],
        accent: (255, 69, 0),
        symbol: "bubble.left.and.bubble.right.fill",
        media: false,
    },
    WebSite {
        key: "gmail",
        name: "Gmail",
        hosts: &["mail.google.com", "gmail.com"],
        hints: &["mail.google.com", "gmail"],
        accent: (234, 67, 53),
        symbol: "envelope.fill",
        media: false,
    },
    WebSite {
        key: "google-drive",
        name: "Google Drive",
        hosts: &["drive.google.com"],
        hints: &[],
        accent: (31, 158, 89),
        symbol: "externaldrive.fill",
        media: false,
    },
    WebSite {
        key: "google-calendar",
        name: "Google Calendar",
        hosts: &["calendar.google.com"],
        hints: &[],
        accent: (66, 133, 244),
        symbol: "calendar",
        media: false,
    },
    WebSite {
        key: "google-docs",
        name: "Google Docs",
        hosts: &["docs.google.com"],
        hints: &[],
        accent: (66, 133, 244),
        symbol: "doc.richtext.fill",
        media: false,
    },
    WebSite {
        key: "linkedin",
        name: "LinkedIn",
        hosts: &["linkedin.com"],
        hints: &["linkedin.com", "linkedin"],
        accent: (10, 102, 194),
        symbol: "person.2.fill",
        media: false,
    },
    WebSite {
        key: "x",
        name: "X",
        hosts: &["x.com", "twitter.com"],
        hints: &["twitter.com", "x.com"],
        accent: (28, 28, 30),
        symbol: "number",
        media: false,
    },
    WebSite {
        key: "github",
        name: "GitHub",
        hosts: &["github.com", "github.io"],
        hints: &["github.com", "github"],
        accent: (36, 41, 46),
        symbol: "chevron.left.forwardslash.chevron.right",
        media: false,
    },
    WebSite {
        key: "gitlab",
        name: "GitLab",
        hosts: &["gitlab.com"],
        hints: &["gitlab"],
        accent: (252, 109, 38),
        symbol: "chevron.left.forwardslash.chevron.right",
        media: false,
    },
    WebSite {
        key: "tiktok",
        name: "TikTok",
        hosts: &["tiktok.com"],
        hints: &["tiktok.com", "tiktok"],
        accent: (37, 244, 238),
        symbol: "music.note",
        media: true,
    },
    WebSite {
        key: "pinterest",
        name: "Pinterest",
        hosts: &["pinterest.com"],
        hints: &["pinterest.com", "pinterest"],
        accent: (203, 32, 39),
        symbol: "pin.fill",
        media: false,
    },
    WebSite {
        key: "substack",
        name: "Substack",
        hosts: &["substack.com"],
        hints: &["substack"],
        accent: (255, 103, 0),
        symbol: "newspaper.fill",
        media: false,
    },
    WebSite {
        key: "medium",
        name: "Medium",
        hosts: &["medium.com"],
        hints: &["medium.com"],
        accent: (0, 0, 0),
        symbol: "newspaper.fill",
        media: false,
    },
    WebSite {
        key: "quora",
        name: "Quora",
        hosts: &["quora.com"],
        hints: &["quora"],
        accent: (184, 42, 21),
        symbol: "text.bubble.fill",
        media: false,
    },
    WebSite {
        key: "notion",
        name: "Notion",
        hosts: &["notion.so", "notion.site"],
        hints: &["notion.so"],
        accent: (0, 0, 0),
        symbol: "doc.text.fill",
        media: false,
    },
    WebSite {
        key: "figma",
        name: "Figma",
        hosts: &["figma.com"],
        hints: &["figma.com", "figma"],
        accent: (162, 89, 255),
        symbol: "square.fill",
        media: false,
    },
];

/// The platform a host belongs to, if any. A host matches exactly or as a
/// subdomain (`www.youtube.com` matches `youtube.com`).
pub fn site_for_host(host: &str) -> Option<&'static WebSite> {
    let h = host.trim().to_lowercase();
    let h = h.trim_start_matches("www.");
    SITES.iter().find(|s| {
        s.hosts.iter().any(|suffix| {
            let suffix = suffix.trim_start_matches("www.");
            h == suffix || h.ends_with(&format!(".{suffix}"))
        })
    })
}

/// The platform a notification's text points at. Hosts are the strong signal
/// (macOS puts the push origin in the subtitle); names are distinctive enough
/// to match as plain substrings. Matches are boundary-aware, so `x.com` does
/// not fire inside `netflix.com` or `max.com`.
pub fn site_from_text(text: &str) -> Option<&'static WebSite> {
    let t = text.to_lowercase();
    SITES.iter().find(|s| s.hosts.iter().chain(s.hints.iter()).any(|needle| contains_word(&t, needle)))
}

/// `needle` in `hay`, only where it is a standalone token: not glued to
/// letters or digits on either side (`.` counts as a boundary, so
/// `www.youtube.com` still matches `youtube.com`).
fn contains_word(hay: &str, needle: &str) -> bool {
    let b = hay.as_bytes();
    let n = needle.len();
    let mut from = 0;
    while let Some(i) = hay[from..].find(needle) {
        let at = from + i;
        let before = at == 0 || !b[at - 1].is_ascii_alphanumeric();
        let after = at + n >= b.len() || !b[at + n].is_ascii_alphanumeric();
        if before && after {
            return true;
        }
        from = at + 1;
    }
    false
}

/// A platform by its key.
pub fn site(key: &str) -> Option<&'static WebSite> {
    SITES.iter().find(|s| s.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_match_subdomains() {
        assert_eq!(site_for_host("youtube.com").map(|s| s.key), Some("youtube"));
        assert_eq!(site_for_host("www.youtube.com").map(|s| s.key), Some("youtube"));
        assert_eq!(site_for_host("music.youtube.com").map(|s| s.key), Some("youtube-music"));
        assert_eq!(site_for_host("notyoutube.com"), None, "no substring hosts");
        assert_eq!(site_for_host("mail.google.com").map(|s| s.key), Some("gmail"));
    }

    #[test]
    fn text_attribution_prefers_hosts_but_accepts_names() {
        assert_eq!(site_from_text("New subscriber youtube.com").map(|s| s.key), Some("youtube"));
        assert_eq!(site_from_text("someone mentioned you on LinkedIn").map(|s| s.key), Some("linkedin"));
        assert_eq!(site_from_text("Your weekly GitHub digest").map(|s| s.key), Some("github"));
        assert_eq!(site_from_text("New video · YouTube").map(|s| s.key), Some("youtube"));
        assert_eq!(site_from_text("Nothing here"), None);
    }

    #[test]
    fn short_hosts_do_not_fire_inside_other_words() {
        // `x.com` must not light up inside netflix.com / max.com …
        assert_eq!(site_from_text("New episode on netflix.com").map(|s| s.key), Some("netflix"));
        assert_eq!(site_from_text("max.com sale"), None);
        // … but the real site matches, subdomain included.
        assert_eq!(site_from_text("https://x.com/user").map(|s| s.key), Some("x"));
        assert_eq!(site_from_text("www.youtube.com via Chrome").map(|s| s.key), Some("youtube"));
    }

    #[test]
    fn every_site_is_complete() {
        for s in SITES {
            assert!(!s.key.is_empty() && !s.name.is_empty());
            assert!(!s.hosts.is_empty(), "{} needs hosts", s.key);
        }
        // Keys are unique and icon-file safe.
        let mut keys: Vec<_> = SITES.iter().map(|s| s.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), SITES.len());
        for k in keys {
            assert!(k.chars().all(|c| c.is_ascii_lowercase() || c == '-'), "key {k}");
        }
    }

    #[test]
    fn lookups_by_key() {
        assert_eq!(site("youtube").map(|s| s.name), Some("YouTube"));
        assert_eq!(site("nope"), None);
    }
}
