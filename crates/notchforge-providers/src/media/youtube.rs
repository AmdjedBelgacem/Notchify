//! YouTube's up-next recommendations, read from the watch page itself.
//!
//! YouTube embeds the sidebar recommendations in the watch page's
//! `ytInitialData` JSON. Fetching the page (no login, no API key) and walking
//! that JSON for `compactVideoRenderer` entries yields the recommended list —
//! robust to layout changes because the walk is recursive rather than
//! path-based. A consent cookie keeps EU requests on the watch page.

pub use notchforge_core::model::UpNextVideo;

/// Fetch the recommendations for a video. Best effort: an empty list on any
/// failure (offline, consent wall, layout change).
pub async fn fetch_up_next(http: &reqwest::Client, video_id: &str) -> Vec<UpNextVideo> {
    let url = format!("https://www.youtube.com/watch?v={video_id}");
    let resp = match http
        .get(&url)
        .header("Cookie", "SOCS=CAI; CONSENT=YES+cb")
        .header("Accept-Language", "en")
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let Ok(html) = resp.text().await else { return Vec::new() };
    parse_up_next(&html, video_id)
}

/// Extract the recommended videos from a watch page's HTML. Pure and tested.
pub fn parse_up_next(html: &str, watching: &str) -> Vec<UpNextVideo> {
    let Some(data) = extract_initial_data(html) else { return Vec::new() };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) else { return Vec::new() };
    let mut pairs = Vec::new();
    collect_renderers(&value, &mut pairs);
    // Never list the video being watched, and cap the list.
    pairs.retain(|(id, _)| id != watching);
    pairs.truncate(8);
    pairs.into_iter().map(|(_, v)| v).collect()
}

/// Find a video by a title, for browsers whose tabs we cannot read: the
/// window title ("Song - YouTube") is all we have. Best effort.
pub async fn search_first(http: &reqwest::Client, query: &str) -> Option<UpNextVideo> {
    search_list(http, query).await.into_iter().next()
}

/// The page's results, capped at eight.
pub async fn search_list(http: &reqwest::Client, query: &str) -> Vec<UpNextVideo> {
    let Some(url) = reqwest::Url::parse_with_params("https://www.youtube.com/results", &[("search_query", query)]).ok()
    else {
        return Vec::new();
    };
    let resp = match http
        .get(url)
        .header("Cookie", "SOCS=CAI; CONSENT=YES+cb")
        .header("Accept-Language", "en")
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let Ok(html) = resp.text().await else { return Vec::new() };
    parse_search_list(&html)
}

/// The first video of a results page. Pure and tested.
pub fn parse_search(html: &str) -> Option<UpNextVideo> {
    parse_search_list(html).into_iter().next()
}

/// Every video on a results page (deduped, at most eight). Pure and tested.
pub fn parse_search_list(html: &str) -> Vec<UpNextVideo> {
    let Some(data) = extract_initial_data(html) else { return Vec::new() };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) else { return Vec::new() };
    let mut pairs = Vec::new();
    collect_renderers(&value, &mut pairs);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (_, v) in pairs {
        if seen.insert(v.url.clone()) {
            out.push(v);
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// Slice out the `ytInitialData` JSON object.
fn extract_initial_data(html: &str) -> Option<String> {
    let start_key = "var ytInitialData = ";
    let start = html.find(start_key)? + start_key.len();
    let rest = &html[start..];
    // Walk to the balanced end of the JSON object, respecting strings.
    let bytes = rest.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(rest[..=i].to_owned());
                }
            }
            _ => {}
        }
    }
    None
}

/// Recursively gather recommended videos in the data tree, in both of
/// YouTube's shapes: the classic `compactVideoRenderer` and the current
/// `lockupViewModel`.
fn collect_renderers(value: &serde_json::Value, out: &mut Vec<(String, UpNextVideo)>) {
    match value {
        serde_json::Value::Object(map) => {
            // Search results use `videoRenderer`, watch-page sidebars the
            // compact variant — the fields we read are the same.
            if let Some(node) = map.get("videoRenderer").or_else(|| map.get("compactVideoRenderer")) {
                if let Some(pair) = video_from_compact(node) {
                    out.push(pair);
                }
            }
            if let Some(node) = map.get("lockupViewModel") {
                if let Some(pair) = video_from_lockup(node) {
                    out.push(pair);
                }
            }
            for v in map.values() {
                collect_renderers(v, out);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                collect_renderers(v, out);
            }
        }
        _ => {}
    }
}

fn video_from_compact(node: &serde_json::Value) -> Option<(String, UpNextVideo)> {
    let video_id = node.get("videoId")?.as_str()?.to_owned();
    let title = text_of(node.get("title"))?;
    // Watch-page sidebars say `authorText`, search results `ownerText`.
    let author = text_of(node.get("authorText")).or_else(|| text_of(node.get("ownerText"))).unwrap_or_default();
    let duration = text_of(node.get("lengthText")).unwrap_or_default();
    Some((
        video_id.clone(),
        UpNextVideo { title, author, duration, url: format!("https://www.youtube.com/watch?v={video_id}"), thumb: None },
    ))
}

/// The current `lockupViewModel` shape: `contentId` is the video, the title
/// lives in the metadata view model, the channel in the first metadata row.
fn video_from_lockup(node: &serde_json::Value) -> Option<(String, UpNextVideo)> {
    let video_id = node.get("contentId")?.as_str()?.to_owned();
    let meta = node.get("metadata")?.get("lockupMetadataViewModel")?;
    let title = meta.get("title")?.get("content")?.as_str()?.to_owned();
    let mut author = String::new();
    let mut duration = String::new();
    if let Some(rows) = meta
        .get("metadata")
        .and_then(|m| m.get("contentMetadataViewModel"))
        .and_then(|c| c.get("metadataRows"))
        .and_then(|r| r.as_array())
    {
        // First row is the channel; the rest are views and age.
        let parts = rows.first().and_then(|r| r.get("metadataParts")).and_then(|p| p.as_array());
        for part in parts.into_iter().flatten() {
            if let Some(t) = part.get("text").and_then(|t| t.get("content")).and_then(|t| t.as_str()) {
                author = t.to_owned();
                break;
            }
        }
    }
    if let Some(overlays) = node
        .get("contentImage")
        .and_then(|c| c.get("thumbnailViewModel"))
        .and_then(|t| t.get("overlays"))
        .and_then(|o| o.as_array())
    {
        'find: for overlay in overlays {
            for badge in overlay
                .get("thumbnailBottomOverlayViewModel")
                .and_then(|b| b.get("badges"))
                .and_then(|b| b.as_array())
                .into_iter()
                .flatten()
            {
                if let Some(text) = badge.get("text").and_then(|t| t.as_str()) {
                    // A duration is digits and colons; "Now playing" is not.
                    if text.contains(':') && text.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                        duration = text.to_owned();
                        break 'find;
                    }
                }
            }
        }
    }
    Some((
        video_id.clone(),
        UpNextVideo { title, author, duration, url: format!("https://www.youtube.com/watch?v={video_id}"), thumb: None },
    ))
}

/// `{"simpleText": "…"}` or `{"runs": [{"text": "…"}, …]}`.
fn text_of(value: Option<&serde_json::Value>) -> Option<String> {
    let v = value?;
    if let Some(t) = v.get("simpleText").and_then(|t| t.as_str()) {
        return Some(t.to_owned());
    }
    Some(v.get("runs")?.as_array()?.iter().filter_map(|r| r.get("text").and_then(|t| t.as_str())).collect::<String>())
}

use std::time::Duration;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_recommended_list() {
        // Minimal watch page: the JSON blob plus decoy scripts around it.
        let page = r#"<html><script>var foo = {"a": 1};</script>
<script>var ytInitialData = {"contents":{"twoColumnWatchNextResults":{"secondaryResults":{"secondaryResults":{"items":[
{"compactVideoRenderer":{"videoId":"vid1","title":{"simpleText":"Next video"},"authorText":{"simpleText":"Channel A"},"lengthText":{"simpleText":"8:12"}}},
{"compactVideoRenderer":{"videoId":"vid2","title":{"runs":[{"text":"Another "}]},"authorText":{"simpleText":"B"},"lengthText":{"simpleText":"12:00"}}},
{"compactVideoRenderer":{"videoId":"watching-now","title":{"simpleText":"The current video"}}}
]}}}}};</script></html>"#;
        let list = parse_up_next(page, "watching-now");
        assert_eq!(list.len(), 2, "the watched video is excluded: {list:?}");
        assert_eq!(list[0].title, "Next video");
        assert_eq!(list[0].author, "Channel A");
        assert_eq!(list[0].duration, "8:12");
        assert_eq!(list[0].url, "https://www.youtube.com/watch?v=vid1");
        assert_eq!(list[1].title, "Another ");
    }

    #[test]
    fn nested_and_broken_pages_degrade_to_empty() {
        // Renderers nested deeply still count.
        let page = r#"<script>var ytInitialData = {"x":{"y":{"items":[{"lockupViewModel":{"contentId":"nope"}},{"compactVideoRenderer":{"videoId":"deep","title":{"simpleText":"Deep"}}}]}}};</script>"#;
        let list = parse_up_next(page, "other");
        assert_eq!(list.len(), 1);
        assert!(list[0].url.ends_with("v=deep"));
        // Missing blob / invalid JSON: empty, never a panic.
        assert!(parse_up_next("<html>no data</html>", "x").is_empty());
        assert!(parse_up_next("<script>var ytInitialData = {broken", "x").is_empty());
    }

    #[test]
    fn parses_search_results_for_a_title() {
        // A minimal results page: the classic `videoRenderer` shape.
        let page = r#"<script>var ytInitialData = {"contents":{"twoColumnSearchResultsRenderer":{"primaryContents":{"sectionListRenderer":{"contents":[{"itemSectionRenderer":{"contents":[
        {"videoRenderer":{"videoId":"abc12345678","title":{"runs":[{"text":"Daft Punk - One More Time (Official Video)"}]},"ownerText":{"runs":[{"text":"Daft Punk"}]},"lengthText":{"simpleText":"4:11"}}},
        {"videoRenderer":{"videoId":"def23456789","title":{"simpleText":"Second"}}}
        ]}}]}}}}};</script>"#;
        let v = parse_search(page).expect("first result");
        assert_eq!(v.title, "Daft Punk - One More Time (Official Video)");
        assert_eq!(v.author, "Daft Punk");
        assert_eq!(v.duration, "4:11");
        assert_eq!(v.url, "https://www.youtube.com/watch?v=abc12345678");
        assert!(parse_search("<html>nothing</html>").is_none());
    }

    #[test]
    #[ignore = "live: run with --ignored and network; parses today's real layout"]
    fn parses_a_real_watch_page() {
        let html = std::fs::read_to_string("/tmp/yt-page.html").expect("save a watch page first");
        let list = parse_up_next(&html, "dQw4w9WgXcQ");
        assert!(!list.is_empty(), "no recommendations parsed");
        assert!(list.iter().all(|v| v.url.starts_with("https://www.youtube.com/watch?v=")));
        assert!(list.iter().any(|v| !v.title.is_empty()));
        println!("parsed {} recommendations, first: {} — {}", list.len(), list[0].title, list[0].author);
    }

    #[test]
    #[ignore = "live: network; run with -- --ignored"]
    fn finds_a_video_by_its_title() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let v = rt.block_on(async {
            let http = reqwest::Client::builder()
                .user_agent(concat!("NotchForge/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap();
            search_first(
                &http,
                &std::env::var("YT_QUERY").unwrap_or_else(|_| "Daft Punk One More Time official video".into()),
            )
            .await
        });
        let v = v.expect("search found a video");
        assert_eq!(v.url.len(), "https://www.youtube.com/watch?v=".len() + 11);
        assert!(!v.title.is_empty());
        println!("found: {} — {}", v.title, v.author);
    }

    #[test]
    #[ignore = "live: network; run with -- --ignored"]
    fn fetches_recommendations_for_a_real_video() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let list = rt.block_on(async {
            let http = reqwest::Client::builder()
                .user_agent(concat!("NotchForge/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap();
            fetch_up_next(&http, "dQw4w9WgXcQ").await
        });
        assert!(!list.is_empty(), "no recommendations fetched");
        assert!(list.iter().all(|v| v.url.starts_with("https://www.youtube.com/watch?v=")));
        println!("fetched {} up next, first: {} — {}", list.len(), list[0].title, list[0].author);
    }
}
