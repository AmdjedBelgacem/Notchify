//! AppleScript bridge to Spotify and Apple Music.
//!
//! Both apps expose a scripting dictionary. Scripts run through `osascript`
//! (so the provider stays free of AppKit), always guarded by a process check so
//! a script never *launches* a player. macOS asks the user once per player for
//! Automation permission; a refusal surfaces as error -1743.
//!
//! Numbers cross the boundary as integers (milliseconds, 0–100) because
//! AppleScript formats reals with the user's locale (`12,5` in French).

use notchforge_core::model::{MediaAction, MediaPlayer, NowPlaying, PlaybackState, Playlist};
use std::time::Duration;

/// Field separator inside script output (ASCII unit separator).
pub const SEP: char = '\u{1f}';

/// Normal scripts finish in well under a second; the long timeout only matters
/// while macOS shows the Automation prompt (the script waits for the answer).
pub const TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptError {
    /// errAEEventNotPermitted (-1743): Automation permission denied.
    NotPermitted,
    /// procNotFound (-600) / the app quit while we asked.
    NotRunning,
    TimedOut,
    Failed(String),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptError::NotPermitted => write!(f, "Automation permission denied"),
            ScriptError::NotRunning => write!(f, "player not running"),
            ScriptError::TimedOut => write!(f, "script timed out"),
            ScriptError::Failed(e) => write!(f, "{e}"),
        }
    }
}

/// Classify `osascript` stderr.
pub fn classify_error(stderr: &str) -> ScriptError {
    if stderr.contains("-1743") || stderr.to_ascii_lowercase().contains("not authorized to send apple events") {
        ScriptError::NotPermitted
    } else if stderr.contains("-600") || stderr.contains("-609") || stderr.contains("isn’t running") {
        ScriptError::NotRunning
    } else {
        ScriptError::Failed(stderr.trim().lines().last().unwrap_or("osascript failed").to_owned())
    }
}

/// Run an AppleScript with arguments (available as `argv` in `on run argv`).
pub async fn run(script: &str, args: &[&str]) -> Result<String, ScriptError> {
    let mut cmd = tokio::process::Command::new("/usr/bin/osascript");
    cmd.arg("-e").arg(script);
    cmd.args(args);
    cmd.stdin(std::process::Stdio::null()).kill_on_drop(true);
    let out = match tokio::time::timeout(TIMEOUT, cmd.output()).await {
        Err(_) => return Err(ScriptError::TimedOut),
        Ok(Err(e)) => return Err(ScriptError::Failed(e.to_string())),
        Ok(Ok(o)) => o,
    };
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end_matches(['\n', '\r']).to_owned())
    } else {
        Err(classify_error(&String::from_utf8_lossy(&out.stderr)))
    }
}

/// Shared handler: text of a value, `""` for `missing value`.
const PRELUDE: &str = r#"
on txt(v)
	if v is missing value then return ""
	try
		return v as string
	on error
		return ""
	end try
end txt
"#;

/// Output: `state␟title␟artist␟album␟duration_ms␟position_ms␟track_id␟artwork_url␟shuffle␟repeat␟volume`
/// or `stopped`.
pub fn state_script(player: MediaPlayer) -> String {
    let body = match player {
        MediaPlayer::Spotify => {
            r#"
	tell application id "com.spotify.client"
		set s to my txt(player state)
		if s is "stopped" then return "stopped"
		set pos to 0
		try
			set pos to (player position * 1000) as integer
		end try
		set t to current track
		set dur to 0
		try
			set dur to (duration of t) as integer
		end try
		return s & d & my txt(name of t) & d & my txt(artist of t) & d & my txt(album of t) & d & dur & d & pos & d & my txt(id of t) & d & my txt(artwork url of t) & d & my txt(shuffling) & d & my txt(repeating) & d & my txt(sound volume)
	end tell"#
        }
        MediaPlayer::Music => {
            r#"
	tell application id "com.apple.Music"
		set s to my txt(player state)
		if s is "stopped" then return "stopped"
		set pos to 0
		try
			set pos to (player position * 1000) as integer
		end try
		set sh to my txt(shuffle enabled)
		set rp to my txt(song repeat)
		set vol to my txt(sound volume)
		try
			set t to current track
		on error
			set st to ""
			try
				set st to current stream title
			end try
			return s & d & my txt(st) & d & "" & d & "" & d & 0 & d & pos & d & "" & d & "" & d & sh & d & rp & d & vol
		end try
		set dur to 0
		try
			set dur to ((duration of t) * 1000) as integer
		end try
		set pid to ""
		try
			set pid to persistent ID of t
		end try
		return s & d & my txt(name of t) & d & my txt(artist of t) & d & my txt(album of t) & d & dur & d & pos & d & pid & d & "" & d & sh & d & rp & d & vol
	end tell"#
        }
    };
    format!("{PRELUDE}\non run\n\tset d to character id 31{body}\nend run")
}

/// AppleScript state constants as text ("playing") or raw codes («kPSP»).
/// The raw codes are case-sensitive: `kPSP` playing, `kPSp` paused, `kPSS` stopped.
pub fn parse_state(s: &str) -> PlaybackState {
    let raw = s.trim();
    match raw.to_ascii_lowercase().as_str() {
        "playing" | "fast forwarding" | "rewinding" => PlaybackState::Playing,
        "paused" => PlaybackState::Paused,
        _ if raw.contains("kPSP") || raw.contains("kPSF") || raw.contains("kPSR") => PlaybackState::Playing,
        _ if raw.contains("kPSp") => PlaybackState::Paused,
        _ => PlaybackState::Stopped,
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" => Some(true),
        "false" | "no" => Some(false),
        // Music's `song repeat`: off / one / all.
        "off" => Some(false),
        "one" | "all" => Some(true),
        _ => None,
    }
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

/// Parse [`state_script`] output. `None` means the player is stopped.
pub fn parse_now_playing(player: MediaPlayer, out: &str, now_ms: i64) -> Option<NowPlaying> {
    let out = out.trim();
    if out.is_empty() || out == "stopped" {
        return None;
    }
    let f: Vec<&str> = out.split(SEP).collect();
    let get = |i: usize| f.get(i).copied().unwrap_or("");
    let state = parse_state(get(0));
    if state == PlaybackState::Stopped {
        return None;
    }
    let ms = |s: &str| s.trim().parse::<i64>().ok().filter(|v| *v > 0).map(|v| v as f64 / 1000.0);
    Some(NowPlaying {
        player,
        state,
        title: get(1).trim().to_owned(),
        artist: get(2).trim().to_owned(),
        album: get(3).trim().to_owned(),
        duration: ms(get(4)),
        position: Some(ms(get(5)).unwrap_or(0.0)),
        sampled_at_ms: now_ms,
        track_id: non_empty(get(6)),
        artwork_path: None,
        shuffle: parse_bool(get(8)),
        repeat: parse_bool(get(9)),
        volume: get(10).trim().parse::<f64>().ok().map(|v| v.clamp(0.0, 100.0) as u8),
    })
    .filter(|n| !n.title.is_empty() || !n.artist.is_empty())
}

/// The Spotify artwork URL from [`state_script`] output.
pub fn artwork_url(out: &str) -> Option<String> {
    out.split(SEP).nth(7).and_then(non_empty).filter(|u| u.starts_with("https://"))
}

/// Export the current Music track's artwork to the path in `argv[1]`.
/// Prints `ok` or `none`.
pub const MUSIC_ARTWORK_SCRIPT: &str = r#"
on run argv
	set f to POSIX file (item 1 of argv)
	tell application id "com.apple.Music"
		try
			set a to raw data of artwork 1 of current track
		on error
			return "none"
		end try
	end tell
	set fh to open for access f with write permission
	try
		set eof fh to 0
		write a to fh
		close access fh
	on error e
		close access fh
		error e
	end try
	return "ok"
end run
"#;

/// Lists user playlists (no folders / system lists): `id␟name␟count` per line.
/// `argv[1]` is the maximum number of playlists.
pub const MUSIC_PLAYLISTS_SCRIPT: &str = r#"
on run argv
	set maxN to (item 1 of argv) as integer
	set d to character id 31
	set out to {}
	tell application id "com.apple.Music"
		set ps to every user playlist whose special kind is none
		set n to 0
		repeat with p in ps
			set n to n + 1
			if n > maxN then exit repeat
			set c to 0
			try
				set c to count of tracks of p
			end try
			set end of out to ((persistent ID of p) as string) & d & ((name of p) as string) & d & c
		end repeat
	end tell
	set AppleScript's text item delimiters to linefeed
	return out as string
end run
"#;

pub fn parse_music_playlists(out: &str) -> Vec<Playlist> {
    out.lines()
        .filter_map(|line| {
            let mut f = line.split(SEP);
            let id = non_empty(f.next()?)?;
            let name = non_empty(f.next()?)?;
            let count = f.next().and_then(|c| c.trim().parse::<u32>().ok());
            Some(Playlist {
                player: MediaPlayer::Music,
                id,
                name,
                track_count: count,
                owner: None,
                artwork_url: None,
                artwork_path: None,
            })
        })
        .collect()
}

/// Script and arguments implementing `action`. Values travel as `argv` so
/// nothing user-controlled is spliced into script source.
pub fn control(player: MediaPlayer, action: &MediaAction) -> (String, Vec<String>) {
    let app = player.bundle_id();
    let tell = |body: &str| format!("on run argv\n\ttell application id \"{app}\"\n\t\t{body}\n\tend tell\nend run");
    let flag = |b: bool| if b { "true" } else { "false" }.to_owned();
    match (player, action) {
        (_, MediaAction::PlayPause) => (tell("playpause"), vec![]),
        (_, MediaAction::Next) => (tell("next track"), vec![]),
        (_, MediaAction::Previous) => (tell("previous track"), vec![]),
        (_, MediaAction::Seek(secs)) => (
            tell("set player position to ((item 1 of argv) as integer)"),
            vec![(secs.max(0.0).round() as i64).to_string()],
        ),
        (_, MediaAction::SetVolume(v)) => {
            (tell("set sound volume to ((item 1 of argv) as integer)"), vec![(*v).min(100).to_string()])
        }
        (MediaPlayer::Spotify, MediaAction::SetShuffle(b)) => {
            (tell("set shuffling to ((item 1 of argv) is \"true\")"), vec![flag(*b)])
        }
        (MediaPlayer::Music, MediaAction::SetShuffle(b)) => {
            (tell("set shuffle enabled to ((item 1 of argv) is \"true\")"), vec![flag(*b)])
        }
        (MediaPlayer::Spotify, MediaAction::SetRepeat(b)) => {
            (tell("set repeating to ((item 1 of argv) is \"true\")"), vec![flag(*b)])
        }
        (MediaPlayer::Music, MediaAction::SetRepeat(b)) => (
            tell(
                "if (item 1 of argv) is \"true\" then\n\t\t\tset song repeat to all\n\t\telse\n\t\t\tset song repeat to off\n\t\tend if",
            ),
            vec![flag(*b)],
        ),
        (MediaPlayer::Spotify, MediaAction::PlayPlaylist(uri)) => {
            (tell("play track (item 1 of argv)"), vec![uri.clone()])
        }
        (MediaPlayer::Music, MediaAction::PlayPlaylist(id)) => {
            (tell("play (first user playlist whose persistent ID is (item 1 of argv))"), vec![id.clone()])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(fields: &[&str]) -> String {
        fields.join(&SEP.to_string())
    }

    #[test]
    fn parses_spotify_state() {
        let out = line(&[
            "playing",
            "Teardrop",
            "Massive Attack",
            "Mezzanine",
            "330773",
            "42500",
            "spotify:track:67Hna13dNDkZvBpTXRIaOJ",
            "https://i.scdn.co/image/ab67616d0000b273",
            "false",
            "true",
            "65",
        ]);
        let n = parse_now_playing(MediaPlayer::Spotify, &out, 1000).expect("playing");
        assert_eq!(n.state, PlaybackState::Playing);
        assert_eq!(n.title, "Teardrop");
        assert_eq!(n.duration, Some(330.773));
        assert_eq!(n.position, Some(42.5));
        assert_eq!(n.track_id.as_deref(), Some("spotify:track:67Hna13dNDkZvBpTXRIaOJ"));
        assert_eq!((n.shuffle, n.repeat, n.volume), (Some(false), Some(true), Some(65)));
        assert_eq!(artwork_url(&out).as_deref(), Some("https://i.scdn.co/image/ab67616d0000b273"));
    }

    #[test]
    fn parses_music_state_and_edge_cases() {
        let out = line(&["paused", "Song", "Artist", "", "200000", "0", "ABCDEF0123456789", "", "true", "all", "100"]);
        let n = parse_now_playing(MediaPlayer::Music, &out, 0).expect("paused");
        assert_eq!(n.state, PlaybackState::Paused);
        assert_eq!(n.subtitle(), "Artist");
        assert_eq!(n.repeat, Some(true));
        assert!(artwork_url(&out).is_none());
        assert!(parse_now_playing(MediaPlayer::Music, "stopped", 0).is_none());
        assert!(parse_now_playing(MediaPlayer::Music, "", 0).is_none());
        // An ad / empty track carries no title or artist.
        let empty = line(&["playing", "", "", "", "0", "0", "", "", "", "", ""]);
        assert!(parse_now_playing(MediaPlayer::Spotify, &empty, 0).is_none());
    }

    #[test]
    fn raw_state_codes() {
        assert_eq!(parse_state("«constant ****kPSP»"), PlaybackState::Playing);
        assert_eq!(parse_state("«constant ****kPSp»"), PlaybackState::Paused);
        assert_eq!(parse_state("kPSS"), PlaybackState::Stopped);
    }

    #[test]
    fn parses_playlists() {
        let out = format!("{}\n{}\n\n", line(&["AAA", "Chill", "34"]), line(&["BBB", "Road trip", "x"]));
        let p = parse_music_playlists(&out);
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].id.as_str(), p[0].name.as_str(), p[0].track_count), ("AAA", "Chill", Some(34)));
        assert_eq!(p[1].track_count, None);
    }

    #[test]
    fn errors_are_classified() {
        assert_eq!(
            classify_error("execution error: Not authorized to send Apple events to Spotify. (-1743)"),
            ScriptError::NotPermitted
        );
        assert_eq!(
            classify_error("execution error: Spotify got an error: Application isn’t running. (-600)"),
            ScriptError::NotRunning
        );
        assert!(matches!(classify_error("syntax error"), ScriptError::Failed(_)));
    }

    #[test]
    fn controls_pass_values_as_arguments() {
        let (s, a) = control(MediaPlayer::Music, &MediaAction::PlayPlaylist("x\" & do shell script \"rm".into()));
        assert!(s.contains("item 1 of argv"));
        assert!(!s.contains("rm"), "argument never spliced into source");
        assert_eq!(a.len(), 1);
        let (_, a) = control(MediaPlayer::Spotify, &MediaAction::Seek(61.6));
        assert_eq!(a, ["62"]);
        let (s, a) = control(MediaPlayer::Music, &MediaAction::SetRepeat(true));
        assert!(s.contains("song repeat to all"));
        assert_eq!(a, ["true"]);
        assert!(state_script(MediaPlayer::Spotify).contains("com.spotify.client"));
    }
}
