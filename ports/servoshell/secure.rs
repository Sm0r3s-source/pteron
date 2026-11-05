/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pteron: warns before opening a page over plain `http` (anyone on the network can read or
//! change it). The warning page offers the secure `https` version or "continue anyway", which
//! is remembered for that site until Pteron is closed.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};

use url::Url;

fn allowed() -> &'static Mutex<HashSet<String>> {
    static ALLOWED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    ALLOWED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// One-time codes handed out with each warning page. "Continue anyway" only works with a code
/// from a warning page that was just shown, so a web page cannot silence the warning by
/// loading `servo:http-continue` itself (it cannot read the warning page to get a code).
fn tokens() -> &'static Mutex<Vec<String>> {
    static TOKENS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    TOKENS.get_or_init(|| Mutex::new(Vec::new()))
}

fn issue_token() -> String {
    let token = crate::util::random_token();
    let mut list = crate::locks::LockExt::lock_ok(tokens());
    if list.len() >= 32 {
        list.remove(0);
    }
    list.push(token.clone());
    token
}

fn use_token(token: &str) -> bool {
    let mut list = crate::locks::LockExt::lock_ok(tokens());
    match list.iter().position(|t| t == token) {
        Some(at) => {
            list.remove(at);
            true
        },
        None => false,
    }
}

pub(crate) fn allow_host(host: &str) {
    if let Ok(mut set) = allowed().lock() {
        set.insert(host.to_ascii_lowercase());
    }
}

fn is_allowed(host: &str) -> bool {
    allowed().lock().map(|set| set.contains(&host.to_ascii_lowercase())).unwrap_or(false)
}

/// Is this host on the user's own computer or network, where `http` is normal?
fn is_private_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => {
                v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
            },
            IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified() || (v6.segments()[0] & 0xfe00) == 0xfc00,
        };
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    !host.contains('.') ||
        host == "localhost" ||
        host.ends_with(".localhost") ||
        host.ends_with(".local") ||
        host.ends_with(".lan") ||
        host.ends_with(".internal") ||
        host.ends_with(".home.arpa")
}

/// Should this navigation be held back (warning, or a check for a secure version first)?
pub(crate) fn needs_warning(url: &Url) -> bool {
    let settings = crate::pteron::settings();
    if url.scheme() != "http" || !(settings.https_warning || settings.https_first) {
        return false;
    }
    match url.host_str() {
        Some(host) => !is_private_host(host) && !is_allowed(host),
        None => false,
    }
}

fn encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

// ---------------------------------------------------------------- https first

/// Hosts already tried: `true` when they answer over https.
fn probed() -> &'static Mutex<std::collections::HashMap<String, bool>> {
    static PROBED: OnceLock<Mutex<std::collections::HashMap<String, bool>>> = OnceLock::new();
    PROBED.get_or_init(Default::default)
}

#[derive(Debug, PartialEq)]
enum Next {
    /// Go straight to the https version.
    Secure,
    /// Show the warning (or load http when the warning is off).
    Warn,
    /// Find out first whether https works.
    Check,
}

fn decide(https_first: bool, known: Option<bool>) -> Next {
    match (https_first, known) {
        (false, _) | (_, Some(false)) => Next::Warn,
        (true, Some(true)) => Next::Secure,
        (true, None) => Next::Check,
    }
}

/// What to load instead of the plain `http` page `url` (see [`needs_warning`]).
pub(crate) fn instead_of(url: &Url) -> Option<Url> {
    let host = url.host_str()?.to_ascii_lowercase();
    let known = crate::locks::LockExt::lock_ok(probed()).get(&host).copied();
    match decide(crate::pteron::settings().https_first, known) {
        Next::Secure => secure_version(url),
        Next::Warn => warning_or_page(url),
        Next::Check => check_page(url, host),
    }
}

/// The warning page, or (with the warning switched off) the site itself.
fn warning_or_page(url: &Url) -> Option<Url> {
    if crate::pteron::settings().https_warning {
        warning_url(url)
    } else {
        allow_host(url.host_str()?);
        Some(url.clone())
    }
}

/// The page shown once the check is done: on to https, or the warning.
fn result_html(url: &Url, secure: bool) -> String {
    let go = match secure_version(url) {
        Some(https) if secure => https.to_string(),
        _ if crate::pteron::settings().https_warning => return warning_html(&warning_url(url).unwrap_or_else(|| url.clone())),
        _ => {
            allow_host(url.host_str().unwrap_or_default());
            url.to_string()
        },
    };
    let go = escape(&go);
    format!("<!doctype html><title>Opening...</title><meta http-equiv=\"refresh\" content=\"0;url={go}\"><p><a href=\"{go}\">Continue</a></p>")
}

/// Shows a short "checking" page and, in the background, finds out whether the site answers
/// over https. The page then moves on by itself.
fn check_page(url: &Url, host: String) -> Option<Url> {
    let key = format!("h{}", crate::util::fnv_hex(url.as_str()));
    let path = crate::youtube::page_path(&key)?;
    if crate::youtube::is_ready(&key) {
        crate::youtube::write_page(&key, &crate::youtube::loading_html("Checking for a secure version...", &key));
        let target = url.clone();
        let job_key = key.clone();
        crate::youtube::spawn_job(&key, move || {
            let secure = crate::updater::probe_https(&host);
            crate::locks::LockExt::lock_ok(probed()).insert(host, secure);
            crate::youtube::write_page(&job_key, &result_html(&target, secure));
        });
    }
    Url::from_file_path(path).ok()
}

/// The address of the warning page for `target`.
pub(crate) fn warning_url(target: &Url) -> Option<Url> {
    Url::parse(&format!("servo:http-warning?url={}", encode(target.as_str()))).ok()
}

fn target_from(url: &Url) -> Option<Url> {
    let (_, value) = url.query_pairs().find(|(key, _)| key == "url")?;
    let target = Url::parse(&value).ok()?;
    (target.scheme() == "http").then_some(target)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The same address over https.
pub(crate) fn secure_version(target: &Url) -> Option<Url> {
    let mut secure = target.clone();
    secure.set_scheme("https").ok()?;
    // A port that was meant for http makes no sense for https.
    let _ = secure.set_port(None);
    Some(secure)
}

/// The page shown instead of an `http` site.
pub(crate) fn warning_html(page_url: &Url) -> String {
    let Some(target) = target_from(page_url) else {
        return "<!doctype html><title>Not secure</title><p>This address cannot be opened.</p>".to_string();
    };
    let host = target.host_str().unwrap_or("this site").to_string();
    let secure = secure_version(&target).map(|u| u.to_string()).unwrap_or_default();
    format!(
        "<!doctype html><title>Not secure</title>{}<h1>This site is not secure</h1>\
<p class=sub>{} uses plain <b>http</b>. Other people on the same network (a caf&eacute;, a shared Wi-Fi) can read \
what you send and change what you see. Do not type passwords or card numbers here.</p>\
<div class=card><h2>What do you want to do?</h2>\
<p><a class=btn href=\"{}\">Try the secure version (https)</a></p>\
<p><a href=\"servo:http-continue?url={}&amp;t={}\">Continue to the site anyway</a> (remembered until you close Pteron)</p>\
<p><a href=\"servo:newtab\">Go to the new tab page</a></p></div>\
<p class=sub>You can turn this warning off in Settings &rarr; Privacy &amp; data.</p>",
        crate::pteron::PAGE_STYLE_PUBLIC,
        escape(&host),
        escape(&secure),
        encode(target.as_str()),
        issue_token(),
    )
}

/// `servo:http-continue?url=...`: remember the site and go on to it.
pub(crate) fn continue_html(page_url: &Url) -> String {
    let Some(target) = target_from(page_url) else {
        return "<!doctype html><title>Not secure</title><p>This address cannot be opened.</p>".to_string();
    };
    let token = page_url.query_pairs().find(|(key, _)| key == "t").map(|(_, v)| v.to_string());
    if !token.as_deref().is_some_and(use_token) {
        return "<!doctype html><title>Not secure</title><p>Use the button on the warning page to continue.</p>".to_string();
    }
    if let Some(host) = target.host_str() {
        allow_host(host);
    }
    let address = escape(target.as_str());
    format!(
        "<!doctype html><title>Opening...</title><meta http-equiv=\"refresh\" content=\"0;url={address}\">\
<p><a href=\"{address}\">Continue</a></p>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warns(url: &str) -> bool {
        needs_warning(&Url::parse(url).unwrap())
    }

    #[test]
    fn warns_only_for_public_http_sites() {
        assert!(warns("http://example.com/page"));
        assert!(warns("http://93.184.216.34/"));
        assert!(!warns("https://example.com/"));
        assert!(!warns("http://localhost:8080/"));
        assert!(!warns("http://127.0.0.1:3000/s/x"));
        assert!(!warns("http://192.168.1.10/"));
        assert!(!warns("http://10.0.0.5/admin"));
        assert!(!warns("http://nas/"));
        assert!(!warns("http://printer.local/"));
        assert!(!warns("http://[::1]/"));
        assert!(!warns("servo:newtab"));
        assert!(!warns("file:///C:/x.html"));
    }

    #[test]
    fn continuing_remembers_the_site() {
        let target = Url::parse("http://remember-me.example/a?b=1&c=2").unwrap();
        assert!(needs_warning(&target));
        let page = warning_url(&target).unwrap();
        let html = warning_html(&page);
        assert!(html.contains("https://remember-me.example/a?b=1&amp;c=2"));
        // Without the code from the warning page, "continue" does nothing.
        let bare = Url::parse(&format!("servo:http-continue?url={}", encode(target.as_str()))).unwrap();
        assert!(!continue_html(&bare).contains("refresh"));
        let wrong = Url::parse(&format!("servo:http-continue?url={}&t=nope", encode(target.as_str()))).unwrap();
        assert!(!continue_html(&wrong).contains("refresh"));
        assert!(needs_warning(&target));
        // The warning page's own link carries a working one-time code.
        let start = html.find("http-continue?").unwrap();
        let end = html[start..].find('"').unwrap() + start;
        let link = html[start..end].replace("&amp;", "&");
        let go = Url::parse(&format!("servo:{link}")).unwrap();
        assert!(continue_html(&go).contains("refresh"));
        assert!(!needs_warning(&target));
        // ...which works only once.
        assert!(!continue_html(&go).contains("refresh"));
        assert!(!needs_warning(&Url::parse("http://REMEMBER-ME.example/other").unwrap()));
    }

    #[test]
    fn warning_page_refuses_odd_addresses() {
        let bad = Url::parse("servo:http-warning?url=javascript%3Aalert(1)").unwrap();
        assert!(warning_html(&bad).contains("cannot be opened"));
        assert!(!continue_html(&bad).contains("refresh"));
        let other = Url::parse("servo:http-continue?url=https%3A%2F%2Fexample.com").unwrap();
        assert!(!continue_html(&other).contains("refresh"));
        let _ = issue_token();
    }

    #[test]
    fn https_first_decisions() {
        assert_eq!(decide(true, None), Next::Check);
        assert_eq!(decide(true, Some(true)), Next::Secure);
        assert_eq!(decide(true, Some(false)), Next::Warn);
        assert_eq!(decide(false, None), Next::Warn);
        assert_eq!(decide(false, Some(true)), Next::Warn);
    }

    #[test]
    fn the_result_page_moves_on() {
        let site = Url::parse("http://old.example:8080/a?b=1").unwrap();
        let html = result_html(&site, true);
        assert!(html.contains("refresh") && html.contains("url=https://old.example/a?b=1"));
        let html = result_html(&site, false);
        assert!(html.contains("not secure") && html.contains("http-continue"));
    }

    #[test]
    fn secure_version_drops_the_port() {
        let t = Url::parse("http://example.com:8080/x").unwrap();
        assert_eq!(secure_version(&t).unwrap().as_str(), "https://example.com/x");
    }
}
