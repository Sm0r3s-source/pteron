/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Sirocco: Plume's extras. Subscriptions without an account (YouTube's public channel feeds,
//! with import and export), chapters, subtitles, SponsorBlock and the remembered player
//! switches. The Plume pages reach these through the local video helper (`/p/<secret>?x=...`).

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::innertube::{self, FeedEntry, VideoItem};
use crate::youtube::{self, esc, valid_channel_id, valid_video_id};

/// One command from a Plume page: `(content type, body)`.
pub(crate) fn command(value: &str) -> (&'static str, String) {
    let json = |v: serde_json::Value| ("application/json", v.to_string());
    if let Some(id) = value.strip_prefix("next:") {
        return json(next_json(id));
    }
    if let Some(rest) = value.strip_prefix("cc:") {
        return match captions_vtt(rest) {
            Ok(vtt) => ("text/vtt", vtt),
            Err(message) => ("text/plain", format!("ERROR {message}")),
        };
    }
    if let Some(id) = value.strip_prefix("sb:") {
        return json(sponsor_segments(id));
    }
    if let Some(id) = value.strip_prefix("sub:has:") {
        return ("text/plain", if youtube::is_subscribed(id) { "1" } else { "0" }.into());
    }
    if let Some(rest) = value.strip_prefix("sub:toggle:") {
        let (id, name) = rest.split_once('|').unwrap_or((rest, ""));
        let on = youtube::toggle_subscription(id, name);
        if on {
            forget_feed();
        }
        return ("text/plain", if on { "1" } else { "0" }.into());
    }
    match value {
        "feed" => json(feed_json()),
        "subs:import" => json(import_subscriptions()),
        "subs:export" => json(export_subscriptions()),
        "pref:get" => {
            let p = youtube::player_prefs();
            json(serde_json::json!({"autonext": p.autonext, "loop": p.looping, "captions": p.captions}))
        },
        _ => {
            if let Some(rest) = value.strip_prefix("pref:") {
                set_pref(rest);
            }
            ("text/plain", String::new())
        },
    }
}

fn set_pref(rest: &str) {
    let Some((key, value)) = rest.split_once(':') else { return };
    let on = value == "1";
    let lang: String = value.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(20).collect();
    youtube::set_player_pref(|p| match key {
        "autonext" => p.autonext = on,
        "loop" => p.looping = on,
        "cc" => p.captions = lang,
        _ => {},
    });
}

// ---------------------------------------------------------------- up next

/// The "Up next" list, asked for after the player is already showing: `{"html", "first"}`.
fn next_json(id: &str) -> serde_json::Value {
    if !valid_video_id(id) {
        return serde_json::json!({"html": "", "first": null});
    }
    match innertube::related(id) {
        Ok(items) => serde_json::json!({
            "html": items.iter().map(youtube::card).collect::<String>(),
            "first": items.first().map(|i| i.id.clone()),
        }),
        Err(message) => serde_json::json!({"html": "", "first": null, "error": message}),
    }
}

// ---------------------------------------------------------------- chapters

/// Chapters written in a description the way YouTube reads them: lines that start with a time,
/// the first one at 0:00, at least three, each later than the one before.
pub(crate) fn chapters(description: &str) -> Vec<(u64, String)> {
    let mut list: Vec<(u64, String)> = Vec::new();
    for line in description.lines() {
        let line = line.trim().trim_start_matches(|c: char| matches!(c, '-' | '–' | '•' | '*' | '[' | '(' | '▶' | ' '));
        let Some((secs, rest)) = leading_time(line) else { continue };
        let title: String = rest
            .trim_start_matches(|c: char| c == ']' || c == ')' || c == '-' || c == '–' || c == ':' || c == '|' || c.is_whitespace())
            .trim()
            .chars()
            .take(100)
            .collect();
        if title.is_empty() {
            continue;
        }
        if list.last().is_some_and(|(last, _)| secs <= *last) {
            continue;
        }
        list.push((secs, title));
    }
    if list.len() >= 3 && list[0].0 == 0 { list } else { Vec::new() }
}

/// `1:23`, `01:23` or `1:02:03` at the start of `text`: the seconds and the rest of the text.
fn leading_time(text: &str) -> Option<(u64, &str)> {
    let end = text.find(|c: char| !(c.is_ascii_digit() || c == ':')).unwrap_or(text.len());
    let token = &text[..end];
    let secs = parse_time(token)?;
    Some((secs, &text[end..]))
}

pub(crate) fn parse_time(token: &str) -> Option<u64> {
    let parts: Vec<&str> = token.split(':').collect();
    if !(2..=3).contains(&parts.len()) || parts.iter().any(|p| p.is_empty() || p.len() > 2) {
        return None;
    }
    let numbers: Vec<u64> = parts.iter().map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let (h, m, s) = match numbers[..] {
        [m, s] => (0, m, s),
        [h, m, s] => (h, m, s),
        _ => return None,
    };
    (s < 60 && (parts.len() == 2 || m < 60)).then_some(h * 3600 + m * 60 + s)
}

/// Turns times in already escaped description text (outside links) into links that seek the
/// video: `<a class="ts" href="#t=83" data-t="83">1:23</a>`.
pub(crate) fn link_times(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let mut rest = html;
    while !rest.is_empty() {
        // Copy links as they are.
        if rest.starts_with("<a ") {
            let end = rest.find("</a>").map(|e| e + 4).unwrap_or(rest.len());
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        let ch = rest.chars().next().unwrap_or(' ');
        let boundary = out.chars().last().is_none_or(|c| !(c.is_alphanumeric() || c == ':' || c == '/' || c == '.'));
        if ch.is_ascii_digit() && boundary {
            let end = rest.find(|c: char| !(c.is_ascii_digit() || c == ':')).unwrap_or(rest.len());
            let token = &rest[..end];
            let after_ok = rest[end..].chars().next().is_none_or(|c| !c.is_alphanumeric());
            if token.contains(':') && after_ok && let Some(secs) = parse_time(token.trim_end_matches(':')) {
                let token = token.trim_end_matches(':');
                out.push_str(&format!("<a class=\"ts\" href=\"#t={secs}\" data-t=\"{secs}\">{token}</a>"));
                rest = &rest[token.len()..];
                continue;
            }
        }
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

// ---------------------------------------------------------------- subtitles

/// `cc:<video id>:<track number>`: the track as WebVTT.
fn captions_vtt(rest: &str) -> Result<String, String> {
    let (id, number) = rest.split_once(':').ok_or("bad request")?;
    let number: usize = number.parse().map_err(|_| "bad request")?;
    if !valid_video_id(id) {
        return Err("bad request".into());
    }
    let info = innertube::player(id)?;
    let track = info.captions.get(number).ok_or("no such subtitles")?;
    let text = innertube::get_text(&format!("{}&fmt=vtt", track.url))?;
    if !text.trim_start_matches('\u{feff}').starts_with("WEBVTT") {
        return Err("YouTube did not send subtitles.".into());
    }
    Ok(text)
}

// ---------------------------------------------------------------- SponsorBlock

const SPONSOR_CATEGORIES: &str = "[\"sponsor\",\"selfpromo\",\"interaction\"]";

/// Segments of `id` marked by SponsorBlock users: `[{"s": start, "e": end, "c": category}]`.
/// Only the first four characters of the video's SHA-256 are sent, so the service cannot tell
/// which video it is; the answer covers every video starting with them.
fn sponsor_segments(id: &str) -> serde_json::Value {
    if !crate::pteron::settings().sponsorblock || !valid_video_id(id) {
        return serde_json::json!([]);
    }
    let Ok(hash) = crate::updater::sha256_bytes(id.as_bytes()) else { return serde_json::json!([]) };
    let categories: String = url::form_urlencoded::byte_serialize(SPONSOR_CATEGORIES.as_bytes()).collect();
    let url = format!("https://sponsor.ajay.app/api/skipSegments/{}?categories={categories}", &hash[..4]);
    match innertube::get_text(&url) {
        Ok(text) => parse_sponsor_segments(&text, id),
        Err(_) => serde_json::json!([]),
    }
}

pub(crate) fn parse_sponsor_segments(text: &str, id: &str) -> serde_json::Value {
    let all: serde_json::Value = serde_json::from_str(text).unwrap_or(serde_json::Value::Null);
    let mut out = Vec::new();
    for video in all.as_array().into_iter().flatten() {
        if video.get("videoID").and_then(|v| v.as_str()) != Some(id) {
            continue;
        }
        for segment in video.get("segments").and_then(|s| s.as_array()).into_iter().flatten() {
            let times = segment.get("segment").and_then(|t| t.as_array());
            let (Some(start), Some(end)) = (
                times.and_then(|t| t.first()).and_then(|t| t.as_f64()),
                times.and_then(|t| t.get(1)).and_then(|t| t.as_f64()),
            ) else {
                continue;
            };
            let category = segment.get("category").and_then(|c| c.as_str()).unwrap_or("sponsor");
            if end > start + 0.5 && matches!(category, "sponsor" | "selfpromo" | "interaction") {
                out.push(serde_json::json!({"s": start, "e": end, "c": category}));
            }
        }
    }
    serde_json::Value::Array(out)
}

// ---------------------------------------------------------------- subscriptions

/// The merged feed is kept a little while, so going back to the home page is instant.
static FEED: Mutex<Option<(Instant, serde_json::Value)>> = Mutex::new(None);
const FEED_TIME: Duration = Duration::from_secs(15 * 60);

fn forget_feed() {
    if let Ok(mut feed) = FEED.lock() {
        *feed = None;
    }
}

/// The newest videos of every followed channel, newest first: `{"html", "count", "channels"}`.
fn feed_json() -> serde_json::Value {
    if let Ok(feed) = FEED.lock() &&
        let Some((when, value)) = feed.as_ref() &&
        when.elapsed() < FEED_TIME
    {
        return value.clone();
    }
    let subs = youtube::subscriptions();
    if subs.is_empty() {
        return serde_json::json!({"html": "", "count": 0, "channels": 0});
    }
    // At most eight feeds at a time.
    let ids: Vec<String> = subs.iter().map(|(id, _)| id.clone()).collect();
    let mut entries: Vec<FeedEntry> = Vec::new();
    for batch in ids.chunks(8) {
        let jobs: Vec<_> = batch
            .iter()
            .cloned()
            .map(|id| std::thread::spawn(move || innertube::channel_feed(&id).unwrap_or_default()))
            .collect();
        for job in jobs {
            entries.extend(job.join().unwrap_or_default());
        }
    }
    let value = serde_json::json!({
        "html": feed_cards(entries, now_secs()),
        "count": subs.len(),
        "channels": subs.len(),
    });
    if let Ok(mut feed) = FEED.lock() {
        *feed = Some((Instant::now(), value.clone()));
    }
    value
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Newest first, at most 60 cards.
pub(crate) fn feed_cards(mut entries: Vec<FeedEntry>, now: i64) -> String {
    entries.sort_by(|a, b| b.published.cmp(&a.published));
    entries.dedup_by(|a, b| a.id == b.id);
    entries
        .iter()
        .take(60)
        .map(|e| {
            youtube::card(&VideoItem {
                id: e.id.clone(),
                title: e.title.clone(),
                channel: e.channel.clone(),
                duration_secs: None,
                views: String::new(),
                published: iso_seconds(&e.published).map(|t| ago(now - t)).unwrap_or_default(),
            })
        })
        .collect()
}

/// `2026-11-15T10:00:00+00:00` as seconds since 1970 (the time zone part is ignored: YouTube
/// sends UTC).
pub(crate) fn iso_seconds(text: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (y, m, d) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hh, mm, ss) = (number(11..13).unwrap_or(0), number(14..16).unwrap_or(0), number(17..19).unwrap_or(0));
    // Days from 1970-01-01 (Howard Hinnant's civil-days algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// "3 hours ago" style text (the card sorts and shows it like YouTube's own).
pub(crate) fn ago(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (n, unit) = match seconds {
        s if s < 3600 => ((s / 60).max(1), "minute"),
        s if s < 86_400 => (s / 3600, "hour"),
        s if s < 7 * 86_400 => (s / 86_400, "day"),
        s if s < 30 * 86_400 => (s / (7 * 86_400), "week"),
        s if s < 365 * 86_400 => (s / (30 * 86_400), "month"),
        s => (s / (365 * 86_400), "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// Channels in a file from YouTube (Google Takeout's `subscriptions.csv`) or another app (OPML).
pub(crate) fn parse_subscription_file(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    if text.contains("<outline") {
        for outline in text.split("<outline").skip(1) {
            let tag = outline.split('>').next().unwrap_or("");
            let attr = |name: &str| {
                let start = tag.find(&format!("{name}=\""))? + name.len() + 2;
                let end = tag[start..].find('"')? + start;
                Some(tag[start..end].replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'"))
            };
            let id = attr("xmlUrl").and_then(|u| u.split("channel_id=").nth(1).map(|i| i.chars().take(24).collect::<String>()));
            if let Some(id) = id.filter(|id| valid_channel_id(id)) {
                out.push((id, attr("title").or_else(|| attr("text")).unwrap_or_default()));
            }
        }
    } else {
        for line in text.lines() {
            let mut fields = line.splitn(3, ',');
            let id = fields.next().unwrap_or("").trim().trim_matches('"');
            let _url = fields.next();
            let name = fields.next().unwrap_or("").trim().trim_matches('"');
            if valid_channel_id(id) {
                out.push((id.to_string(), name.to_string()));
            }
        }
    }
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

fn import_subscriptions() -> serde_json::Value {
    let chosen = match crate::appearance::choose_file("Import subscriptions", "Subscriptions", &["csv", "opml", "xml"]) {
        Ok(Some(path)) => path,
        Ok(None) => return serde_json::json!({"added": 0, "cancelled": true}),
        Err(message) => return serde_json::json!({"added": 0, "error": message}),
    };
    let Ok(text) = std::fs::read_to_string(&chosen) else {
        return serde_json::json!({"added": 0, "error": "Could not read that file."});
    };
    let list = parse_subscription_file(&text);
    if list.is_empty() {
        return serde_json::json!({"added": 0, "error": "No YouTube channels found in that file."});
    }
    let added = youtube::add_subscriptions(list);
    forget_feed();
    serde_json::json!({"added": added})
}

pub(crate) fn subscriptions_opml(subs: &[(String, String)]) -> String {
    let mut text = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"1.1\">\n<head><title>Plume subscriptions</title></head>\n<body>\n<outline text=\"YouTube Subscriptions\" title=\"YouTube Subscriptions\">\n",
    );
    for (id, name) in subs {
        text.push_str(&format!(
            "<outline text=\"{0}\" title=\"{0}\" type=\"rss\" xmlUrl=\"https://www.youtube.com/feeds/videos.xml?channel_id={1}\"/>\n",
            esc(name),
            id
        ));
    }
    text.push_str("</outline>\n</body>\n</opml>\n");
    text
}

pub(crate) fn subscriptions_csv(subs: &[(String, String)]) -> String {
    let mut text = String::from("Channel Id,Channel Url,Channel Title\n");
    for (id, name) in subs {
        text.push_str(&format!("{id},http://www.youtube.com/channel/{id},{}\n", name.replace([',', '\n', '"'], " ")));
    }
    text
}

/// Saves both formats in the Downloads folder.
fn export_subscriptions() -> serde_json::Value {
    let subs = youtube::subscriptions();
    let dir = dirs::download_dir().or_else(|| dirs::home_dir().map(|h| h.join("Downloads")));
    let Some(dir) = dir else { return serde_json::json!({"error": "No Downloads folder."}) };
    let _ = std::fs::create_dir_all(&dir);
    let opml = dir.join("Plume-subscriptions.opml");
    let csv = dir.join("Plume-subscriptions.csv");
    match std::fs::write(&opml, subscriptions_opml(&subs)).and_then(|_| std::fs::write(&csv, subscriptions_csv(&subs))) {
        Ok(()) => serde_json::json!({"path": opml.display().to_string(), "count": subs.len()}),
        Err(error) => serde_json::json!({"error": error.to_string()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_chapters_like_youtube() {
        let text = "Intro text\n0:00 Intro\n1:23 - First part\n12:05 Second part\n1:02:03 The end\nnot a chapter 5:00 here";
        assert_eq!(
            chapters(text),
            vec![(0, "Intro".into()), (83, "First part".into()), (725, "Second part".into()), (3723, "The end".into())]
        );
        assert!(chapters("1:00 a\n2:00 b\n3:00 c").is_empty(), "must start at 0:00");
        assert!(chapters("0:00 a\n2:00 b").is_empty(), "needs three");
        assert_eq!(parse_time("1:60"), None);
        assert_eq!(parse_time("99:59"), Some(5999));
    }

    #[test]
    fn links_times_but_not_inside_links() {
        let html = "See 1:23 and <a href=\"https://x.example/a?t=1:23\">https://x.example/a?t=1:23</a> or 10:00. Ratio 16:9x stays.";
        let out = link_times(html);
        assert!(out.contains("<a class=\"ts\" href=\"#t=83\" data-t=\"83\">1:23</a> and"));
        assert!(out.contains("<a class=\"ts\" href=\"#t=600\" data-t=\"600\">10:00</a>."));
        assert!(out.contains("href=\"https://x.example/a?t=1:23\""), "links stay as they were");
        assert!(out.contains("16:9x"));
    }

    #[test]
    fn reads_sponsor_segments_for_the_right_video() {
        let text = r#"[{"videoID":"aaaaaaaaaaa","segments":[{"segment":[1,2],"category":"sponsor"}]},
            {"videoID":"dQw4w9WgXcQ","segments":[{"segment":[10.5,40.0],"category":"sponsor"},{"segment":[50,51],"category":"music_offtopic"}]}]"#;
        let segments = parse_sponsor_segments(text, "dQw4w9WgXcQ");
        assert_eq!(segments, serde_json::json!([{"s": 10.5, "e": 40.0, "c": "sponsor"}]));
        assert_eq!(parse_sponsor_segments("not json", "dQw4w9WgXcQ"), serde_json::json!([]));
    }

    #[test]
    fn feeds_and_dates() {
        let xml = r#"<feed><title>Some Channel</title><entry><yt:videoId>dQw4w9WgXcQ</yt:videoId><title>Old &amp; good</title>
            <author><name>Some Channel</name></author><published>2026-11-10T10:00:00+00:00</published></entry>
            <entry><yt:videoId>aaaaaaaaaaa</yt:videoId><title>New</title><author><name>Some Channel</name></author>
            <published>2026-11-15T10:00:00+00:00</published></entry><entry><yt:videoId>bad</yt:videoId></entry></feed>"#;
        let entries = innertube::parse_feed(xml);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "Old & good");
        let now = iso_seconds("2026-11-16T10:00:00+00:00").unwrap();
        let html = feed_cards(entries, now);
        assert!(html.find("aaaaaaaaaaa").unwrap() < html.find("dQw4w9WgXcQ").unwrap(), "newest first");
        assert!(html.contains("1 day ago") && html.contains("6 days ago"));
        assert_eq!(iso_seconds("1970-01-02T00:00:00Z"), Some(86_400));
        assert_eq!(ago(90 * 60), "1 hour ago");
    }

    #[test]
    fn imports_takeout_csv_and_opml() {
        let csv = "Channel Id,Channel Url,Channel Title\nUCuAXFkgsw1L7xaCfnd5JJOw,http://www.youtube.com/channel/UCuAXFkgsw1L7xaCfnd5JJOw,Some, Name\nbad,line\n";
        assert_eq!(parse_subscription_file(csv), vec![("UCuAXFkgsw1L7xaCfnd5JJOw".into(), "Some, Name".into())]);
        let subs = vec![("UCuAXFkgsw1L7xaCfnd5JJOw".to_string(), "A & B".to_string())];
        let opml = subscriptions_opml(&subs);
        assert_eq!(parse_subscription_file(&opml), vec![("UCuAXFkgsw1L7xaCfnd5JJOw".into(), "A & B".into())]);
        assert_eq!(parse_subscription_file(&subscriptions_csv(&subs)), subs);
    }
}
