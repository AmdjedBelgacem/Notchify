//! Lyrics for the current track, from [LRCLIB](https://lrclib.net) — free, no
//! key, no account. The search endpoint takes a track and artist name and
//! answers with synced (LRC) or plain lyrics; the first result with timings
//! wins, else the first plain one. Best effort: an empty result on any
//! failure (offline, no match).

use notchforge_core::model::Lyrics;

pub async fn fetch(http: &reqwest::Client, title: &str, artist: &str) -> Option<Lyrics> {
    let url = reqwest::Url::parse_with_params(
        "https://lrclib.net/api/search",
        &[("track_name", title), ("artist_name", artist)],
    )
    .ok()?;
    let resp = match http
        .get(url)
        .header("User-Agent", concat!("NotchForge/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return None,
    };
    let Ok(body) = resp.text().await else { return None };
    parse(&body)
}

/// Parse an LRCLIB search response. Pure and tested.
pub fn parse(json: &str) -> Option<Lyrics> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return None };
    let items = v.as_array()?;
    let mut plain_only: Option<Lyrics> = None;
    for item in items {
        let synced = item.get("syncedLyrics").and_then(|s| s.as_str()).unwrap_or_default();
        if !synced.trim().is_empty() {
            let lines = parse_lrc(synced);
            if !lines.is_empty() {
                return Some(Lyrics { lines, plain: false });
            }
        }
        if plain_only.is_none() {
            let plain = item.get("plainLyrics").and_then(|s| s.as_str()).unwrap_or_default();
            if !plain.trim().is_empty() {
                plain_only = Some(Lyrics { lines: plain.lines().map(|l| (0, l.to_owned())).collect(), plain: true });
            }
        }
    }
    plain_only
}

/// LRC timestamps: `[mm:ss.xx]` / `[mm:ss.xxx]`, possibly several per line,
/// with metadata tags (`[ar: …]`) to skip.
pub fn parse_lrc(text: &str) -> Vec<(i64, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut times: Vec<i64> = Vec::new();
        let mut rest = line.trim();
        loop {
            rest = match rest.strip_prefix('[') {
                Some(r) => r,
                None => break,
            };
            let Some(end) = rest.find(']') else { break };
            let tag = &rest[..end];
            match parse_stamp(tag) {
                Some(ms) => times.push(ms),
                None => break, // a metadata tag: the line has no timings
            }
            rest = rest[end + 1..].trim_start();
        }
        if times.is_empty() {
            continue;
        }
        let text = rest.trim().to_owned();
        for ms in times {
            out.push((ms, text.clone()));
        }
    }
    out.sort_by_key(|(ms, _)| *ms);
    out
}

/// `mm:ss` / `mm:ss.xx` / `mm:ss.xxx` → milliseconds.
fn parse_stamp(tag: &str) -> Option<i64> {
    let (m, rest) = tag.split_once(':')?;
    let mins: i64 = m.trim().parse().ok()?;
    let (secs, frac) = match rest.split_once('.') {
        Some((s, f)) => (s.parse::<i64>().ok()?, f),
        None => (rest.trim().parse::<i64>().ok()?, ""),
    };
    // Hundredths or thousandths both land on milliseconds.
    let ms = match frac.len() {
        0 => 0,
        1 | 2 => format!("{frac:0<3}").parse::<i64>().ok()?,
        _ => frac[..3].parse::<i64>().ok()?,
    };
    Some(mins * 60_000 + secs * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = r#"[
      {"trackName":"X","artistName":"A","plainLyrics":"first line\nsecond line"},
      {"trackName":"X","artistName":"A","syncedLyrics":"[ar:A]\n[00:12.50]One\n[00:15]Two\n[01:02.25]Three"}
    ]"#;

    #[test]
    fn synced_beats_plain_and_parses() {
        let l = parse(SEARCH).expect("lyrics");
        assert!(!l.plain);
        assert_eq!(l.lines.len(), 3);
        assert_eq!(l.lines[0], (12_500, "One".into()));
        assert_eq!(l.lines[1], (15_000, "Two".into()));
        assert_eq!(l.lines[2], (62_250, "Three".into()));
        // The line playing at 16 s is "Two"; during the intro nothing is.
        assert_eq!(l.line_at(16_000), Some(1));
        assert_eq!(l.line_at(5_000), None);
        // Gap lines never highlight.
        let with_gap =
            Lyrics { lines: vec![(0, "a".into()), (10_000, "♪".into()), (20_000, "b".into())], plain: false };
        assert_eq!(with_gap.line_at(12_000), Some(0));
        assert_eq!(with_gap.line_at(22_000), Some(2));
    }

    #[test]
    fn plain_falls_back() {
        let only_plain = r#"[{"trackName":"X","plainLyrics":"a\nb"}]"#;
        let l = parse(only_plain).expect("plain");
        assert!(l.plain);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.line_at(1_000_000), None, "plain lyrics never highlight");
        // Broken JSON and empties degrade to None, never a panic.
        assert!(parse("{oops").is_none());
        assert!(parse("[]").is_none());
    }

    #[test]
    fn lrc_shapes() {
        // Multiple timestamps share one line; whole-second and single-digit
        // fraction stamps parse too.
        let l = parse_lrc("[00:01][00:30.5][01:02.25]chorus");
        assert_eq!(l, vec![(1_000, "chorus".into()), (30_500, "chorus".into()), (62_250, "chorus".into())]);
    }
}
