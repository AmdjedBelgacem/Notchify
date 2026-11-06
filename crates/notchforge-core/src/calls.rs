//! Incoming calls.
//!
//! macOS has no public way to see another app's calls, so a call is recognised
//! from the notification announcing it. Only apps that can place calls are
//! considered, and only wording that announces a call, so a message saying
//! "call me later" stays a message. Apps that ring only in their own window
//! send no such notification and can't be seen.

use crate::apps::{AppRef, is_browser};
use serde::{Deserialize, Serialize};

/// How long a call rings without another sign of life before it counts as missed.
pub const RING_TIMEOUT_MS: i64 = 45_000;
/// A call that just ended ignores late copies of its own notification.
pub const ENDED_QUIET_MS: i64 = 30_000;
/// An answered call is over this long after the last sign it's still going.
pub const ACTIVE_GONE_MS: i64 = 15_000;
/// A call seen in an app's window is over this soon after its buttons go.
pub const WINDOW_GONE_MS: i64 = 8_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CallId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallKind {
    Audio,
    Video,
    Meeting,
}

impl CallKind {
    pub fn label(self) -> &'static str {
        match self {
            CallKind::Audio => "Audio call",
            CallKind::Video => "Video call",
            CallKind::Meeting => "Meeting",
        }
    }
}

/// A call ringing now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    pub id: CallId,
    pub caller: String,
    pub kind: CallKind,
    /// "WhatsApp", "Google Meet in Chrome"…
    pub source: String,
    pub app: AppRef,
    pub url: Option<String>,
    /// The title on the native banner, used to find its buttons.
    pub banner_title: String,
    /// The process ringing in its own window, whose buttons answer the call.
    pub window_pid: Option<i32>,
    pub last_seen_ms: i64,
    /// Answer was pressed but the app's button wasn't there to press yet.
    #[serde(default)]
    pub answering: bool,
}

impl Call {
    /// Identifies a call across its notification, its window and a tab whose
    /// title changes while it rings.
    pub fn key(&self) -> String {
        format!("{}|{}", self.app.bundle_id.as_deref().unwrap_or(&self.source), self.caller)
    }

    /// "Léa Martin" → "LM", "Team" → "T".
    pub fn initials(&self) -> String {
        initials_of(&self.caller)
    }
}

/// A call you picked up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveCall {
    pub caller: String,
    pub kind: CallKind,
    pub source: String,
    pub app: AppRef,
    /// The process with its hang-up button, when the call is in a window.
    pub window_pid: Option<i32>,
    pub started_ms: i64,
    pub last_seen_ms: i64,
}

impl ActiveCall {
    /// "Léa Martin" → "LM".
    pub fn initials(&self) -> String {
        initials_of(&self.caller)
    }
}

fn initials_of(name: &str) -> String {
    let letters: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect();
    if letters.is_empty() { "?".to_owned() } else { letters.to_uppercase() }
}

/// How long a call has been going: `"0:42"`, `"12:05"`, `"1:02:09"`.
pub fn call_clock(started_ms: i64, now_ms: i64) -> String {
    let s = ((now_ms - started_ms).max(0) / 1000) as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// A call recognised in a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recognised {
    pub caller: String,
    pub kind: CallKind,
    pub source: String,
}

/// Apps that can place calls: (bundle id, name).
const CALL_APPS: &[(&str, &str)] = &[
    ("com.apple.FaceTime", "FaceTime"),
    ("net.whatsapp.WhatsApp", "WhatsApp"),
    ("WhatsApp", "WhatsApp"),
    ("ru.keepcoder.Telegram", "Telegram"),
    ("org.telegram.desktop", "Telegram"),
    ("org.whispersystems.signal-desktop", "Signal"),
    ("com.tinyspeck.slackmacgap", "Slack"),
    ("com.hnc.Discord", "Discord"),
    ("com.microsoft.teams2", "Microsoft Teams"),
    ("com.microsoft.teams", "Microsoft Teams"),
    ("us.zoom.xos", "Zoom"),
    ("com.skype.skype", "Skype"),
    ("com.facebook.archon", "Messenger"),
    ("com.facebook.archon.developerID", "Messenger"),
    ("com.apple.mobilephone", "Phone"),
    ("com.google.meet", "Google Meet"),
];

/// Every calling app's bundle id.
pub const CALL_APP_BUNDLES: &[&str] = &[
    "com.apple.FaceTime",
    "net.whatsapp.WhatsApp",
    "ru.keepcoder.Telegram",
    "org.telegram.desktop",
    "org.whispersystems.signal-desktop",
    "com.tinyspeck.slackmacgap",
    "com.hnc.Discord",
    "com.microsoft.teams2",
    "com.microsoft.teams",
    "us.zoom.xos",
    "com.skype.skype",
    "com.facebook.archon",
    "com.facebook.archon.developerID",
];

/// The name of a calling app, by bundle id.
pub fn call_app_name(bundle_id: &str) -> Option<&'static str> {
    CALL_APPS.iter().find(|(b, _)| b.eq_ignore_ascii_case(bundle_id)).map(|(_, n)| *n)
}

/// Could this notification be about a call? Decides what diagnostics note.
pub fn looks_call_like(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["call", "appel", "ring", "facetime", "meet", "huddle", "video", "vidéo"].iter().any(|w| lower.contains(w))
}

/// The kind of a call from the words around it.
pub fn kind_from_text(text: &str) -> CallKind {
    let lower = text.to_lowercase();
    if lower.contains("video") || lower.contains("vidéo") {
        CallKind::Video
    } else if lower.contains("meeting") || lower.contains("réunion") {
        CallKind::Meeting
    } else {
        CallKind::Audio
    }
}

/// Text in a ringing window that says what's happening rather than who is calling.
pub fn is_call_wording(text: &str) -> bool {
    let lower = text.to_lowercase();
    action_matches(&lower, ANSWER_WORDS)
        || action_matches(&lower, DECLINE_WORDS)
        || [
            "incoming",
            "calling",
            "appel",
            "ringing",
            "video call",
            "audio call",
            "voice call",
            "whatsapp",
            "zoom",
            "discord",
            "telegram",
            "messenger",
            "instagram",
            "facebook",
            "teams",
            "slack",
        ]
        .iter()
        .any(|w| lower.contains(w))
}

/// Does a window's title itself say a call is ringing, like Discord's
/// "Incoming Call - Discord"? Enough on its own, even with no readable buttons.
pub fn title_says_ringing(title: &str) -> bool {
    let lower = title.to_lowercase();
    [
        "incoming call",
        "incoming video call",
        "incoming voice call",
        "is calling",
        "calling you",
        "appel entrant",
        "vous appelle",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

/// Short on-page wording that means a call is ringing, for diagnostics. Exact
/// phrases only, so ordinary messages stay out.
pub fn says_ringing_on_page(text: &str) -> bool {
    text.chars().count() <= 40 && title_says_ringing(text)
}

/// Does this text talk about a call? Accept and decline buttons alone don't make
/// a call: message and friend requests have them too.
pub fn mentions_call(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["call", "appel", "appelle", "ringing", "video chat", "facetime", "huddle"].iter().any(|w| lower.contains(w))
}

/// Readable text: without invisible direction marks, and with plain spaces.
fn cleaned_text(text: &str) -> String {
    let visible: String = text
        .chars()
        .filter(|c| !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .map(|c| if c == '\u{a0}' { ' ' } else { c })
        .collect();
    visible.trim().to_owned()
}

/// `Léa Martin - WhatsApp voice call` → `Léa Martin`.
fn name_before_wording(text: &str) -> String {
    match text.split_once(" - ") {
        Some((name, rest)) if is_call_wording(rest) => name.trim().to_owned(),
        _ => text.to_owned(),
    }
}

/// `Isl is calling` → `Isl`, `Léa vous appelle` → `Léa`.
fn name_in_ringing_text(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    [" is calling", " vous appelle", " appelle"].iter().find_map(|phrase| {
        // Lowering can change a text's length; only cut where positions agree.
        let at = lower.find(phrase).filter(|_| lower.len() == text.len())?;
        Some(text[..at].trim().to_owned()).filter(|name| !name.is_empty())
    })
}

/// The caller's name among a ringing window's texts.
pub fn caller_from_texts(texts: &[String], fallback: &str) -> String {
    texts
        .iter()
        .map(|t| name_before_wording(&cleaned_text(t)))
        .find(|t| !t.is_empty() && t.chars().count() <= 60 && !is_call_wording(t))
        .or_else(|| texts.iter().find_map(|t| name_in_ringing_text(&cleaned_text(t))))
        .unwrap_or_else(|| fallback.to_owned())
}

/// A window's call whose caller can't be read yet.
pub const UNKNOWN_CALLER: &str = "Unknown caller";

/// Do two call keys mean the same call? A caller who can't be read yet doesn't
/// make a second call from the same app.
pub fn same_call(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (a.split_once('|'), b.split_once('|')) {
        (Some((app_a, who_a)), Some((app_b, who_b))) => {
            app_a == app_b && (who_a == UNKNOWN_CALLER || who_b == UNKNOWN_CALLER)
        }
        _ => false,
    }
}

/// What to look for among a browser's tabs to find a call's, best first.
pub fn call_tab_wanted(caller: &str, source: &str) -> Vec<String> {
    let mut wanted = Vec::new();
    if caller != UNKNOWN_CALLER {
        wanted.push(format!("{caller} is calling"));
        wanted.push(format!("{caller} vous appelle"));
    }
    wanted.extend(call_site_addresses(source).iter().map(|a| (*a).to_owned()));
    wanted.push("is calling".to_owned());
    wanted
}

/// Where a calling site lives: "Messenger in Dia" → facebook.com…
fn call_site_addresses(source: &str) -> &'static [&'static str] {
    let site = source.split(" in ").next().unwrap_or(source);
    match site {
        "Messenger" => &["facebook.com", "messenger.com"],
        "Instagram" => &["instagram.com"],
        "Google Meet" => &["meet.google.com"],
        "WhatsApp" => &["web.whatsapp.com"],
        "Microsoft Teams" => &["teams.microsoft.com", "teams.live.com"],
        "Slack" => &["app.slack.com"],
        "Discord" => &["discord.com"],
        "Zoom" => &["zoom.us"],
        "Telegram" => &["web.telegram.org"],
        _ => &[],
    }
}

/// Sites that place calls in a browser: (what to look for, name).
const CALL_SITES: &[(&str, &str)] = &[
    ("instagram", "Instagram"),
    ("meet.google.com", "Google Meet"),
    ("google meet", "Google Meet"),
    ("web.whatsapp.com", "WhatsApp"),
    ("whatsapp", "WhatsApp"),
    ("messenger.com", "Messenger"),
    ("messenger", "Messenger"),
    ("facebook", "Messenger"),
    ("teams.microsoft.com", "Microsoft Teams"),
    ("microsoft teams", "Microsoft Teams"),
    ("app.slack.com", "Slack"),
    ("slack", "Slack"),
    ("discord", "Discord"),
    ("zoom", "Zoom"),
    ("telegram", "Telegram"),
];

/// The calling site a browser tab's title points to, as "Instagram in Dia".
pub fn browser_call_source(bundle_id: &str, title: &str) -> Option<String> {
    let lower = title.to_lowercase();
    let (_, site) = CALL_SITES.iter().find(|(hint, _)| lower.contains(hint))?;
    Some(format!("{site} in {}", browser_name(bundle_id)))
}

/// What a call in a browser is called when its site isn't one we know.
pub fn browser_source_name(bundle_id: &str) -> String {
    browser_name(bundle_id).to_owned()
}

/// Apps that make sound all day and never ring: not worth reading their windows.
pub fn never_rings(bundle_id: &str) -> bool {
    matches!(
        bundle_id,
        "com.spotify.client" | "com.apple.Music" | "com.apple.Podcasts" | "com.apple.TV" | "com.apple.QuickTimePlayerX"
    ) || bundle_id.starts_with("dev.notchforge.")
}

fn browser_name(bundle_id: &str) -> &'static str {
    match bundle_id {
        "com.google.Chrome" => "Chrome",
        "com.apple.Safari" => "Safari",
        "com.brave.Browser" => "Brave",
        "company.thebrowser.Browser" => "Arc",
        "org.mozilla.firefox" => "Firefox",
        "com.microsoft.edgemac" => "Edge",
        "company.thebrowser.dia" => "Dia",
        _ => "your browser",
    }
}

/// Wording that announces a call that is ringing now. The second part says
/// what kind it is. Notifications in English and French are understood.
const RINGING: &[(&str, CallKind)] = &[
    ("incoming video call", CallKind::Video),
    ("incoming facetime video", CallKind::Video),
    ("facetime video", CallKind::Video),
    ("video call from", CallKind::Video),
    ("appel vidéo entrant", CallKind::Video),
    ("appel video entrant", CallKind::Video),
    ("incoming voice call", CallKind::Audio),
    ("incoming audio call", CallKind::Audio),
    ("incoming facetime audio", CallKind::Audio),
    ("facetime audio", CallKind::Audio),
    ("incoming call", CallKind::Audio),
    ("voice call from", CallKind::Audio),
    ("appel vocal entrant", CallKind::Audio),
    ("appel audio entrant", CallKind::Audio),
    ("appel entrant", CallKind::Audio),
    ("is calling you", CallKind::Audio),
    ("is calling", CallKind::Audio),
    ("calling you", CallKind::Audio),
    ("vous appelle", CallKind::Audio),
    ("wants to meet", CallKind::Meeting),
    ("invited you to join", CallKind::Meeting),
    ("is inviting you to", CallKind::Meeting),
    ("started a huddle", CallKind::Meeting),
    ("wants you to join", CallKind::Meeting),
    ("started a meeting", CallKind::Meeting),
];

/// Wording about a call that is not ringing.
const NOT_RINGING: &[&str] = &["missed", "manqué", "manque", "ended", "terminé", "declined", "refusé", "voicemail"];

/// Cut the announcement out of a line, leaving who is calling.
fn caller_in(line: &str, phrase: &str) -> Option<String> {
    let lower = line.to_lowercase();
    let at = lower.find(phrase)?;
    // Byte positions in the lowered text can differ from the original's when
    // case folding changes a character's length; only cut where they agree.
    let cleaned = |s: &str| s.trim().trim_matches(|c: char| ":,.-–—…".contains(c)).trim().to_owned();
    if lower.len() != line.len() {
        return None;
    }
    let before = cleaned(&line[..at]);
    if !before.is_empty() {
        return Some(before);
    }
    let after = line[at + phrase.len()..].trim_start();
    let after = after.strip_prefix("from ").unwrap_or(after);
    Some(cleaned(after)).filter(|s| !s.is_empty())
}

/// Does this notification announce a call ringing now?
pub fn recognise(app: &AppRef, title: &str, subtitle: Option<&str>, body: &str) -> Option<Recognised> {
    let bundle = app.bundle_id.as_deref().unwrap_or("");
    let text = format!("{title}\n{}\n{body}", subtitle.unwrap_or("")).to_lowercase();
    if NOT_RINGING.iter().any(|w| text.contains(w)) {
        return None;
    }
    let source = if let Some((_, name)) = CALL_APPS.iter().find(|(b, _)| b.eq_ignore_ascii_case(bundle)) {
        (*name).to_owned()
    } else if is_browser(bundle) {
        let (_, site) = CALL_SITES.iter().find(|(hint, _)| text.contains(hint))?;
        format!("{site} in {}", browser_name(bundle))
    } else if bundle.is_empty() {
        // A banner seen through Accessibility doesn't name its app's bundle.
        if text.contains("facetime") { "FaceTime".to_owned() } else { app.display_name.clone() }
    } else {
        return None;
    };
    let &(phrase, mut kind) = RINGING.iter().find(|(p, _)| text.contains(p))?;
    if source.starts_with("Google Meet") {
        kind = CallKind::Meeting;
    }
    let lines = [title, subtitle.unwrap_or(""), body];
    // A site's name is what a browser shows as the title, never who's calling.
    let is_site = |t: &str| t.trim_end().ends_with(':') || t.contains(".com") || t.contains(".net");
    let caller = lines
        .iter()
        .find_map(|l| name_in_ringing_text(&cleaned_text(l)))
        .or_else(|| lines.iter().find(|l| l.to_lowercase().contains(phrase)).and_then(|l| caller_in(l, phrase)))
        .or_else(|| {
            // The announcement stands alone, so the title is the caller.
            Some(title.trim().to_owned()).filter(|t| !t.is_empty() && !t.to_lowercase().contains(phrase) && !is_site(t))
        })
        .unwrap_or_else(|| UNKNOWN_CALLER.to_owned());
    Some(Recognised { caller, kind, source })
}

/// Words a banner's buttons use for answering a call.
pub const ANSWER_WORDS: &[&str] = &["accept", "answer", "join", "pick up", "accepter", "répondre", "rejoindre"];
/// Words a banner's buttons use to expand an inline reply field.
pub const REPLY_WORDS: &[&str] = &["reply", "répondre", "answer", "respond"];
/// Words for ending a call in progress, surest first. "Leave" alone is tried
/// only after these, since other buttons use it too.
pub const HANG_UP_WORDS: &[&str] =
    &["hang up", "end call", "leave call", "disconnect", "raccrocher", "mettre fin", "terminer l'appel"];
pub const LEAVE_WORDS: &[&str] = &["leave", "quitter"];
/// Words a banner's buttons use for declining a call.
pub const DECLINE_WORDS: &[&str] =
    &["decline", "reject", "ignore", "dismiss", "hang up", "end call", "refuser", "ignorer", "raccrocher"];

/// Does a banner action or button with this name match one of `words`?
pub fn action_matches(name: &str, words: &[&str]) -> bool {
    let lower = name.to_lowercase();
    words.iter().any(|w| lower.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(bundle: &str) -> AppRef {
        AppRef { known: None, bundle_id: Some(bundle.to_owned()), display_name: bundle.to_owned(), platform: None }
    }

    #[test]
    fn an_unread_caller_is_not_a_second_call() {
        assert!(same_call("dia|Léa", "dia|Léa"));
        assert!(same_call("dia|Unknown caller", "dia|Léa"));
        assert!(same_call("dia|Léa", "dia|Unknown caller"));
        assert!(!same_call("dia|Léa", "dia|Sam"));
        assert!(!same_call("dia|Unknown caller", "discord|Léa"));
    }

    #[test]
    fn finds_a_calling_site_by_its_address() {
        assert_eq!(call_site_addresses("Messenger in Dia"), ["facebook.com", "messenger.com"]);
        assert_eq!(call_site_addresses("Google Meet in Safari"), ["meet.google.com"]);
        assert!(call_site_addresses("Dia").is_empty());
        assert_eq!(call_tab_wanted(UNKNOWN_CALLER, "Instagram in Dia"), ["instagram.com", "is calling"]);
        assert_eq!(call_tab_wanted("Léa", "Dia")[0], "Léa is calling");
    }

    #[test]
    fn recognises_calls_from_apps() {
        let r = recognise(&app("net.whatsapp.WhatsApp"), "Léa Martin", None, "Incoming video call").expect("call");
        assert_eq!((r.caller.as_str(), r.kind, r.source.as_str()), ("Léa Martin", CallKind::Video, "WhatsApp"));

        let r = recognise(&app("com.tinyspeck.slackmacgap"), "Marc is calling you", None, "").expect("call");
        assert_eq!((r.caller.as_str(), r.kind), ("Marc", CallKind::Audio));

        let r = recognise(&app("com.apple.FaceTime"), "Marc Dubois", Some("FaceTime Audio"), "").expect("call");
        assert_eq!(r.caller, "Marc Dubois");

        let r = recognise(&app("ru.keepcoder.Telegram"), "Incoming call from Sam", None, "").expect("call");
        assert_eq!(r.caller, "Sam");

        let r = recognise(&app("net.whatsapp.WhatsApp"), "Léa", None, "Appel vidéo entrant").expect("appel");
        assert_eq!((r.caller.as_str(), r.kind), ("Léa", CallKind::Video));
    }

    #[test]
    fn recognises_calls_from_browsers() {
        let r = recognise(
            &app("com.google.Chrome"),
            "Team standup",
            Some("meet.google.com"),
            "Maya is inviting you to join",
        )
        .expect("call");
        assert_eq!((r.kind, r.source.as_str()), (CallKind::Meeting, "Google Meet in Chrome"));
    }

    #[test]
    fn facebooks_notification_names_the_caller() {
        let r = recognise(
            &app("company.thebrowser.dia"),
            "facebook.com:",
            Some("Incoming call"),
            "Isl is calling you. Click to answer.",
        )
        .expect("call");
        assert_eq!((r.caller.as_str(), r.source.as_str()), ("Isl", "Messenger in Dia"));
    }

    #[test]
    fn ordinary_messages_are_not_calls() {
        let whatsapp = app("net.whatsapp.WhatsApp");
        assert_eq!(recognise(&whatsapp, "Léa", None, "Call me later?"), None);
        assert_eq!(recognise(&whatsapp, "Léa", None, "Missed video call"), None);
        assert_eq!(recognise(&whatsapp, "Léa", None, "Appel vocal manqué"), None);
        assert_eq!(recognise(&app("com.apple.mail"), "Incoming call from Sam", None, ""), None, "not a calling app");
        assert_eq!(
            recognise(&app("com.google.Chrome"), "News", None, "Incoming call from Sam"),
            None,
            "no calling site"
        );
    }

    #[test]
    fn a_lone_announcement_has_no_name_to_show() {
        let r = recognise(&app("us.zoom.xos"), "Incoming call", None, "").expect("call");
        assert_eq!(r.caller, "Unknown caller");
    }

    #[test]
    fn initials_are_short() {
        let call = |caller: &str| Call {
            id: CallId(1),
            caller: caller.into(),
            kind: CallKind::Audio,
            source: "x".into(),
            app: app("x"),
            url: None,
            banner_title: String::new(),
            window_pid: None,
            last_seen_ms: 0,
            answering: false,
        };
        assert_eq!(call("Léa Martin").initials(), "LM");
        assert_eq!(call("marc anne dubois").initials(), "MA");
        assert_eq!(call(" ").initials(), "?");
    }

    #[test]
    fn titles_can_say_a_call_is_ringing() {
        assert!(title_says_ringing("Incoming Call - Discord"));
        assert!(title_says_ringing("Léa is calling"));
        assert!(!title_says_ringing("Friends - Discord"));
        assert!(!title_says_ringing("Video call with Léa"));
    }

    #[test]
    fn call_clocks_read_like_a_phone() {
        assert_eq!(call_clock(0, 42_000), "0:42");
        assert_eq!(call_clock(0, 725_000), "12:05");
        assert_eq!(call_clock(0, 3_729_000), "1:02:09");
        assert_eq!(call_clock(5_000, 0), "0:00");
    }

    #[test]
    fn media_players_are_left_alone() {
        assert!(never_rings("com.spotify.client"));
        assert!(!never_rings("com.example.SomeNewCallingApp"));
    }

    #[test]
    fn a_call_keeps_its_identity_when_a_tab_renames_it() {
        let call = |source: &str| Call {
            id: CallId(1),
            caller: "Isl".into(),
            kind: CallKind::Audio,
            source: source.into(),
            app: app("company.thebrowser.dia"),
            url: None,
            banner_title: String::new(),
            window_pid: None,
            last_seen_ms: 0,
            answering: false,
        };
        assert_eq!(call("Messenger in Dia").key(), call("Dia").key());
    }

    #[test]
    fn requests_are_not_calls() {
        assert!(mentions_call("Marc is calling you"));
        assert!(mentions_call("\u{200e}Accept call"));
        assert!(mentions_call("Vous appelle"));
        assert!(!mentions_call("Message request from Marc"));
        assert!(!mentions_call("Wants to be your friend"));
    }

    #[test]
    fn browser_tabs_point_to_calling_sites() {
        let dia = "company.thebrowser.dia";
        assert_eq!(browser_call_source(dia, "Instagram • Messages").as_deref(), Some("Instagram in Dia"));
        assert_eq!(browser_call_source(dia, "(2) Messenger").as_deref(), Some("Messenger in Dia"));
        assert_eq!(browser_call_source(dia, "Rust docs"), None);
        assert!(crate::apps::is_browser(dia));
    }

    #[test]
    fn the_caller_is_found_among_a_windows_text() {
        let t = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(caller_from_texts(&t(&["Incoming video call", "Léa Martin", "Accept"]), "?"), "Léa Martin");
        assert_eq!(caller_from_texts(&t(&["WhatsApp", "Decline"]), "Unknown caller"), "Unknown caller");
        // WhatsApp's call window: the caller is in the title, among invisible marks.
        let title = "\u{200e}\u{202a}+33\u{a0}6\u{a0}12\u{a0}34\u{202c} - WhatsApp voice call";
        assert_eq!(caller_from_texts(&t(&[title]), "?"), "+33 6 12 34");
        assert_eq!(caller_from_texts(&t(&["Léa Martin - WhatsApp video call"]), "?"), "Léa Martin");
        assert_eq!(kind_from_text(title), CallKind::Audio);
        assert!(action_matches("\u{200e}Accept call", ANSWER_WORDS));
        assert!(action_matches("hang up", DECLINE_WORDS));
        // A name inside the wording, with or without anything better around it.
        assert_eq!(caller_from_texts(&t(&["Isl is calling"]), "?"), "Isl");
        assert_eq!(caller_from_texts(&t(&["Léa vous appelle"]), "?"), "Léa");
        assert_eq!(caller_from_texts(&t(&["med.islem is calling", "med.islem"]), "?"), "med.islem");
        assert_eq!(kind_from_text("Incoming video call"), CallKind::Video);
        assert_eq!(kind_from_text("Appel entrant"), CallKind::Audio);
    }

    #[test]
    fn banner_buttons_are_matched_by_name() {
        assert!(action_matches("Name:Accept\nTarget:0x0", ANSWER_WORDS));
        assert!(action_matches("Répondre", ANSWER_WORDS));
        assert!(action_matches("Decline", DECLINE_WORDS));
        assert!(!action_matches("Close", ANSWER_WORDS));
    }
}
