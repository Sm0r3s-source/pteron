/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pteron: full ad and tracker filter lists (EasyList and EasyPrivacy) for the ad blocker.
//! The lists are downloaded once a week from easylist.to into the settings folder and read by
//! the Rust `adblock` engine. They cost memory, so this is a separate switch, off by default.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use crate::locks::LockExt;

const LISTS: [(&str, &str); 2] = [
    ("easylist.txt", "https://easylist.to/easylist/easylist.txt"),
    ("easyprivacy.txt", "https://easylist.to/easylist/easyprivacy.txt"),
];
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

static ENGINE: Mutex<Option<adblock::Engine>> = Mutex::new(None);
static LOADING: AtomicBool = AtomicBool::new(false);
static RULES: AtomicUsize = AtomicUsize::new(0);

fn dir() -> Option<PathBuf> {
    ::dirs::config_dir().map(|dir| dir.join(crate::pteron::APP_NAME).join("filters"))
}

/// Turn the lists on (start loading them in the background) or off (free the memory).
pub(crate) fn set_enabled(on: bool) {
    if !on {
        *ENGINE.lock_ok() = None;
        RULES.store(0, Ordering::Relaxed);
        return;
    }
    if ENGINE.lock_ok().is_some() || LOADING.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::Builder::new()
        .name("pteron-filters".to_string())
        .spawn(|| {
            load();
            LOADING.store(false, Ordering::Release);
        })
        .ok();
}

/// A line for the settings page.
pub(crate) fn status() -> String {
    match (RULES.load(Ordering::Relaxed), LOADING.load(Ordering::Relaxed)) {
        (0, true) => "Loading the filter lists...".to_string(),
        (0, false) => "Filter lists are not loaded.".to_string(),
        (rules, _) => format!("{rules} filter rules loaded."),
    }
}

fn load() {
    let Some(dir) = dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let mut rules: Vec<String> = Vec::new();
    for (name, url) in LISTS {
        let path = dir.join(name);
        let old = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_none_or(|age| age > MAX_AGE);
        if old &&
            let Ok(text) = crate::updater::fetch_html(url, name) &&
            looks_like_list(&text)
        {
            let _ = std::fs::write(&path, text);
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            rules.extend(text.lines().map(str::to_string));
        }
    }
    if rules.is_empty() {
        return;
    }
    let count = rules.len();
    let engine = adblock::Engine::from_rules(rules, adblock::lists::ParseOptions::default());
    *ENGINE.lock_ok() = Some(engine);
    RULES.store(count, Ordering::Relaxed);
}

/// A downloaded list starts with an `[Adblock Plus ...]` line; anything else (an error page) is not kept.
fn looks_like_list(text: &str) -> bool {
    text.len() > 10_000 && text.trim_start().starts_with("[Adblock")
}

/// The request kind the engine knows, from the name of Servo's request destination.
pub(crate) fn kind_of(destination: &str) -> &'static str {
    match destination.to_ascii_lowercase().as_str() {
        "script" | "worker" | "sharedworker" | "serviceworker" => "script",
        "style" => "stylesheet",
        "image" => "image",
        "font" => "font",
        "audio" | "video" | "track" => "media",
        "document" | "iframe" | "frame" => "subdocument",
        "object" | "embed" => "object",
        "report" => "csp_report",
        _ => "other",
    }
}

/// Do the filter lists say this request should be cancelled?
pub(crate) fn blocks(url: &url::Url, page: &str, destination: &str) -> bool {
    let guard = ENGINE.lock_ok();
    let Some(engine) = guard.as_ref() else { return false };
    adblock::request::Request::new(url.as_str(), page, kind_of(destination))
        .is_ok_and(|request| engine.check_network_request(&request).matched)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_list_check() {
        assert_eq!(kind_of("Script"), "script");
        assert_eq!(kind_of("Style"), "stylesheet");
        assert_eq!(kind_of("IFrame"), "subdocument");
        assert_eq!(kind_of("Json"), "other");
        assert!(!looks_like_list("<html>Just a moment</html>"));
        assert!(looks_like_list(&format!("[Adblock Plus 2.0]\n{}", "||ads.example^\n".repeat(1000))));
    }

    #[test]
    fn engine_blocks_by_rule_and_respects_exceptions() {
        let rules = ["||ads.example^$third-party", "/banner/*$image", "@@||ads.example/ok^"].map(String::from).to_vec();
        let engine = adblock::Engine::from_rules(rules, adblock::lists::ParseOptions::default());
        *ENGINE.lock_ok() = Some(engine);
        let blocked = |url: &str, kind: &str| blocks(&url::Url::parse(url).unwrap(), "https://news.example/", kind);
        assert!(blocked("https://ads.example/x.js", "Script"));
        assert!(blocked("https://cdn.example/banner/a.png", "Image"));
        assert!(!blocked("https://cdn.example/banner/a.js", "Script"));
        assert!(!blocked("https://ads.example/ok", "Script"));
        assert!(!blocked("https://news.example/story", "Document"));
        set_enabled(false);
        assert!(!blocked("https://ads.example/x.js", "Script"));
    }
}
