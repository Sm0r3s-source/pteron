/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Sirocco: known phishing and malware sites. Two free lists from the malware-filter project
//! (sites URLhaus reports as spreading malware, and phishing sites from PhishTank, OpenPhish
//! and others) are downloaded once a week into the settings folder. A page on a listed site is
//! replaced by a warning; pictures, scripts and frames from such sites are not loaded. Nothing
//! about the sites you visit is sent anywhere: the check is done on your computer.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime};

use url::Url;

use crate::locks::LockExt;

/// File name, then the addresses to try (a mirror if the first is down).
const LISTS: [(&str, [&str; 2]); 2] = [
    (
        "danger-malware.txt",
        [
            "https://malware-filter.gitlab.io/malware-filter/urlhaus-filter-domains-online.txt",
            "https://curbengh.github.io/malware-filter/urlhaus-filter-domains-online.txt",
        ],
    ),
    (
        "danger-phishing.txt",
        [
            "https://malware-filter.gitlab.io/malware-filter/phishing-filter-domains.txt",
            "https://curbengh.github.io/malware-filter/phishing-filter-domains.txt",
        ],
    ),
];
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// Listed host names, sorted (a sorted list takes about half the memory of a hash set).
static SITES: RwLock<Vec<Box<str>>> = RwLock::new(Vec::new());
static ENABLED: AtomicBool = AtomicBool::new(false);
static LOADING: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
/// Sites the user chose to open anyway, until Sirocco closes.
static ALLOWED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn dir() -> Option<PathBuf> {
    ::dirs::config_dir().map(|dir| dir.join(crate::pteron::APP_NAME).join("filters"))
}

/// Turn the check on (load the lists in the background) or off (free the memory).
pub(crate) fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    if !on {
        SITES.write().map(|mut s| *s = Vec::new()).ok();
        COUNT.store(0, Ordering::Relaxed);
        return;
    }
    if COUNT.load(Ordering::Relaxed) > 0 || LOADING.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::Builder::new()
        .name("pteron-danger-lists".to_string())
        .spawn(|| {
            load();
            LOADING.store(false, Ordering::Release);
        })
        .ok();
}

/// A line for the settings page.
pub(crate) fn status() -> String {
    match (COUNT.load(Ordering::Relaxed), LOADING.load(Ordering::Relaxed)) {
        (0, true) => "Loading the lists...".to_string(),
        (0, false) => "The lists are not loaded yet.".to_string(),
        (count, _) => format!("{count} dangerous sites known."),
    }
}

fn load() {
    let Some(dir) = dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let mut sites = Vec::new();
    for (name, addresses) in LISTS {
        let path = dir.join(name);
        let old = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_none_or(|age| age > MAX_AGE);
        if old {
            for address in addresses {
                if let Ok(text) = crate::updater::fetch_html(address, name) &&
                    looks_like_list(&text)
                {
                    let _ = std::fs::write(&path, text);
                    break;
                }
            }
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            sites.extend(parse(&text));
        }
    }
    sites.sort_unstable();
    sites.dedup();
    sites.shrink_to_fit();
    COUNT.store(sites.len(), Ordering::Relaxed);
    if let Ok(mut store) = SITES.write() {
        *store = sites;
    }
}

/// One host name per line (`#` and `!` lines are comments); `||name^` lines are accepted too.
fn parse(text: &str) -> impl Iterator<Item = Box<str>> + '_ {
    text.lines().filter_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            return None;
        }
        let name = line.split_whitespace().next()?.trim_start_matches("||").trim_end_matches('^');
        let name = name.trim_end_matches('.').to_ascii_lowercase();
        (name.contains('.') && !name.contains('/') && name.len() < 254).then(|| name.into_boxed_str())
    })
}

/// A list file, not an error page: mostly host names.
fn looks_like_list(text: &str) -> bool {
    text.len() > 1000 && !text.trim_start().starts_with('<') && parse(text).take(20).count() >= 10
}

/// Is this address on a known phishing or malware site (or a part of one)?
pub(crate) fn is_dangerous(url: &Url) -> bool {
    if !ENABLED.load(Ordering::Relaxed) || !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else { return false };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if ALLOWED.lock_ok().iter().any(|allowed| allowed == host) {
        return false;
    }
    let Ok(sites) = SITES.read() else { return false };
    listed(&sites, host)
}

/// `host` or any domain above it is in the sorted list.
fn listed(sites: &[Box<str>], host: &str) -> bool {
    let mut name = host;
    loop {
        if sites.binary_search_by(|site| (**site).cmp(name)).is_ok() {
            return true;
        }
        match name.split_once('.') {
            Some((_, rest)) if rest.contains('.') => name = rest,
            _ => return false,
        }
    }
}

fn token() -> &'static str {
    static TOKEN: OnceLock<String> = OnceLock::new();
    TOKEN.get_or_init(crate::util::random_token)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

/// The page shown instead of a dangerous site (served at the site's own address, so the address
/// bar shows where you were going).
pub(crate) fn warning_html(url: &Url) -> String {
    let host = url.host_str().unwrap_or("This site");
    format!(
        "<!doctype html><meta charset=utf-8><title>Dangerous site</title>{}\
<h1 style=\"color:#c0392b\">This site is known to be dangerous</h1>\
<p class=sub><b>{}</b> is on public lists of sites that steal passwords and card numbers (phishing) \
or spread harmful programs. Sirocco did not open it.</p>\
<div class=card><h2>What do you want to do?</h2><p><a class=btn href=\"servo:newtab\">Take me somewhere safe</a></p>\
<p>If you are sure this is a mistake: <a href=\"servo:danger-continue?url={}&amp;t={}\">open the site \
anyway</a> (until you close Sirocco). Never type a password there.</p></div>\
<p class=sub>Lists by the malware-filter project (URLhaus, PhishTank, OpenPhish and others), checked on \
your computer. You can turn this off in Settings &rarr; Privacy &amp; data.</p>",
        crate::pteron::PAGE_STYLE_PUBLIC,
        escape(host),
        encode(url.as_str()),
        token(),
    )
}

/// `servo:danger-continue?url=...&t=...`: the user chose to open a listed site anyway.
pub(crate) fn continue_html(page_url: &Url) -> String {
    let pairs: std::collections::HashMap<String, String> = page_url.query_pairs().into_owned().collect();
    let target = pairs.get("url").and_then(|u| Url::parse(u).ok()).filter(|u| matches!(u.scheme(), "http" | "https"));
    let (Some(target), Some(given)) = (target, pairs.get("t")) else {
        return "<!doctype html><title>Dangerous site</title><p>This address cannot be opened.</p>".into();
    };
    if given != token() {
        return "<!doctype html><title>Dangerous site</title><p>Use the link on the warning page.</p>".into();
    }
    if let Some(host) = target.host_str() {
        ALLOWED.lock_ok().push(host.trim_end_matches('.').to_ascii_lowercase());
    }
    let address = escape(target.as_str());
    format!("<!doctype html><title>Opening...</title><meta http-equiv=\"refresh\" content=\"0;url={address}\"><p><a href=\"{address}\">Continue</a></p>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lists_and_matches_subdomains() {
        let text = "# Title: test\n! comment\nevil.example\n||phish.example.org^\nbad.example.net\n\n";
        let mut sites: Vec<Box<str>> = parse(text).collect();
        sites.sort_unstable();
        assert_eq!(sites.len(), 3);
        assert!(listed(&sites, "evil.example"));
        assert!(listed(&sites, "login.evil.example"));
        assert!(listed(&sites, "phish.example.org"));
        assert!(!listed(&sites, "example.org"), "the parent of a listed site is not listed");
        assert!(!listed(&sites, "notevil.example"));
        assert!(!looks_like_list("<html>Just a moment</html>"));
        assert!(looks_like_list(&"bad.example\n".repeat(200)));
    }

    #[test]
    fn continuing_needs_the_code() {
        let target = "https://evil.example/login";
        let bad = Url::parse(&format!("servo:danger-continue?url={}&t=nope", encode(target))).unwrap();
        assert!(continue_html(&bad).contains("Use the link"));
        let good = Url::parse(&format!("servo:danger-continue?url={}&t={}", encode(target), token())).unwrap();
        assert!(continue_html(&good).contains("refresh"));
        assert!(ALLOWED.lock_ok().iter().any(|h| h == "evil.example"));
        let page = warning_html(&Url::parse("https://evil.example/<x>").unwrap());
        assert!(page.contains("evil.example") && !page.contains("<x>"));
    }
}
