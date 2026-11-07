//! The compatibility tab: pages that Servo cannot handle (Cloudflare's browser check and
//! similar) open in the system's own web engine inside a Pteron tab. This file holds the logic
//! (when to switch, which sites are remembered, the placeholder page); `compat_host.rs` holds
//! the embedded engine itself.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use url::Url;

use crate::locks::LockExt;

const SCHEME: &str = "servo";
const PATH: &str = "compat";
const MAX_SITES: usize = 200;
/// A browser check this soon after a hand-over means Servo was not accepted; later, the pass ran out.
const FAIL_WINDOW: Duration = Duration::from_secs(120);

/// `servo:compat?url=<page>`: the tab shows `page` in the system web engine.
pub(crate) fn wrap(target: &Url) -> Option<Url> {
    if !matches!(target.scheme(), "http" | "https") {
        return None;
    }
    let encoded: String = url::form_urlencoded::byte_serialize(target.as_str().as_bytes()).collect();
    Url::parse(&format!("{SCHEME}:{PATH}?url={encoded}")).ok()
}

/// The page behind a `servo:compat?url=...` address.
pub(crate) fn target(url: &Url) -> Option<Url> {
    if url.scheme() != SCHEME || url.path() != PATH {
        return None;
    }
    let (_, value) = url.query_pairs().find(|(key, _)| key == "url")?;
    Url::parse(&value).ok().filter(|u| matches!(u.scheme(), "http" | "https"))
}

/// The address to show in the address bar: the real page for a compatibility tab.
pub(crate) fn display(url: &str) -> String {
    Url::parse(url).ok().and_then(|u| target(&u)).map_or_else(|| url.to_string(), |u| u.to_string())
}

/// Titles of browser-check pages that Servo cannot pass.
pub(crate) fn is_challenge_title(title: &str) -> bool {
    const STARTS: [&str; 5] = [
        "just a moment",
        "attention required",
        "performing security verification",
        "checking your browser",
        "verifying you are human",
    ];
    let title = title.trim().to_ascii_lowercase();
    STARTS.iter().any(|start| title.starts_with(start))
}

/// Host names, lower case, without duplicates, at most [`MAX_SITES`].
pub(crate) fn clean_sites(sites: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    sites
        .into_iter()
        .map(|s| normalize_site(&s))
        .filter(|s| !s.is_empty() && s.len() < 254 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-')))
        .filter(|s| seen.insert(s.clone()))
        .take(MAX_SITES)
        .collect()
}

/// True when `host` is listed, or is a subdomain of a listed site.
pub(crate) fn site_listed(sites: &[String], host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let host = host.trim_end_matches('.').trim_start_matches("www.");
    sites.iter().any(|s| host == *s || host.ends_with(&format!(".{s}")))
}

/// Sites whose browser check the system engine passed and handed to Servo (with the pass cookie).
fn handed() -> &'static Mutex<HashMap<String, Instant>> {
    static HANDED: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    HANDED.get_or_init(Default::default)
}

/// Sites where the hand-over did not hold (Servo got the check again): they stay in the compatibility tab.
fn staying() -> &'static Mutex<HashSet<String>> {
    static STAY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    STAY.get_or_init(Default::default)
}

fn bare(host: &str) -> String {
    host.to_ascii_lowercase().trim_end_matches('.').trim_start_matches("www.").to_string()
}

pub(crate) fn mark_handed_off(host: &str) {
    handed().lock_ok().insert(bare(host), Instant::now());
}

pub(crate) fn handed_off(host: &str) -> bool {
    handed().lock_ok().contains_key(&bare(host))
}

/// A browser check showed up in Servo on a handed-over site: `Some(true)` soon after the hand-over
/// (Servo was not accepted), `Some(false)` later (the pass ran out and a new one is needed),
/// `None` when the site was not handed over. Either way the site is no longer counted as handed over.
pub(crate) fn check_returned(host: &str) -> Option<bool> {
    let at = handed().lock_ok().remove(&bare(host))?;
    Some(at.elapsed() < FAIL_WINDOW)
}

/// The hand-over was switched off: handed-over sites go back to the normal rules.
pub(crate) fn forget_handoffs() {
    handed().lock_ok().clear();
}

/// The hand-over failed for this site: no more attempts this session.
pub(crate) fn mark_stay(host: &str) {
    handed().lock_ok().remove(&bare(host));
    staying().lock_ok().insert(bare(host));
}

pub(crate) fn stays(host: &str) -> bool {
    staying().lock_ok().contains(&bare(host))
}

/// What happens at a browser check. With the hand-over on it is always "auto": the page moves to a
/// compatibility tab only to pass the check and is then handed back to Servo.
pub(crate) fn effective_mode(settings: &crate::pteron::Settings) -> &str {
    if settings.compat_tab_handoff { "auto" } else { &settings.compat_tab_mode }
}

/// A site name from whatever the user typed: a pasted address, with a path, a port or `www.`, is cut down to the host.
pub(crate) fn normalize_site(text: &str) -> String {
    let text = text.trim().to_ascii_lowercase();
    let rest = text.split_once("://").map_or(text.as_str(), |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("");
    host.trim_start_matches("www.").trim_matches('.').to_string()
}

/// Set once the system web engine could not be started; nothing switches to it after that.
static BROKEN: AtomicBool = AtomicBool::new(false);

pub(crate) fn mark_broken() {
    BROKEN.store(true, Ordering::Relaxed);
}

pub(crate) fn usable() -> bool {
    cfg!(pteron_wry) && !BROKEN.load(Ordering::Relaxed)
}

/// A listed site opened by link or address: the compatibility address to load instead.
pub(crate) fn redirect(settings: &crate::pteron::Settings, url: &Url) -> Option<Url> {
    if !usable() || !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?;
    // The "never use Pteron's own engine" list always applies (claude.ai is on it), whatever the mode.
    if site_listed(&settings.compat_tab_keep_sites, host) {
        return wrap(url);
    }
    // A site that was handed over to Servo loads there, with the cookie, until that fails.
    // With the hand-over on, the other list and mode are switched off.
    if handed_off(host) || settings.compat_tab_handoff || effective_mode(settings) == "off" {
        return None;
    }
    site_listed(&settings.compat_tab_sites, host).then(|| wrap(url)).flatten()
}

/// Each (tab, page) switches by itself once: coming back to the check page with Back must not
/// switch again and again.
pub(crate) fn first_switch(tab: &str, page: &str) -> bool {
    static DONE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mut done = DONE.get_or_init(Default::default).lock_ok();
    if done.len() > 500 {
        done.clear();
    }
    done.insert(format!("{tab}|{page}"))
}

/// Each tab's place in its history, to tell Back from Forward.
fn history_places() -> &'static Mutex<HashMap<String, usize>> {
    static PLACES: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    PLACES.get_or_init(Default::default)
}

/// Back or Forward landed on a compatibility tab whose site was handed over to Servo. It would
/// only hand over again, so Back could never get past it: the steps to move on (negative: back,
/// also past the browser-check page before it).
pub(crate) fn history_skip(tab: &str, entries: &[Url], current: usize) -> Option<isize> {
    let last = history_places().lock_ok().insert(tab.to_string(), current)?;
    let page = entries.get(current).and_then(target)?;
    if last == current || current + 1 >= entries.len() || !page.host_str().is_some_and(handed_off) {
        return None;
    }
    let check_before = current
        .checked_sub(1)
        .and_then(|i| entries.get(i))
        .is_some_and(|u| u.host_str() == page.host_str() && u.path() == page.path());
    let back = 1 + usize::from(check_before);
    Some(if last < current || current < back { 1 } else { -(back as isize) })
}

/// Hosts that switched to the compatibility tab by themselves, waiting to go on the list.
fn remembered() -> &'static Mutex<Vec<String>> {
    static PENDING: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    PENDING.get_or_init(Default::default)
}

pub(crate) fn remember_later(host: &str) {
    remembered().lock_ok().push(host.to_string());
}

pub(crate) fn take_remembered() -> Vec<String> {
    std::mem::take(&mut *remembered().lock_ok())
}

/// Sites the user sent back to the compatibility tab after a hand-over looked wrong.
fn keep_pending() -> &'static Mutex<Vec<String>> {
    static KEEP: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    KEEP.get_or_init(Default::default)
}

pub(crate) fn keep_later(host: &str) {
    keep_pending().lock_ok().push(host.to_string());
}

pub(crate) fn take_keep() -> Vec<String> {
    std::mem::take(&mut *keep_pending().lock_ok())
}

/// The site of the last hand-over and when it happened, for the "looks wrong?" notice.
fn last_handoff() -> &'static Mutex<Option<(std::time::Instant, String)>> {
    static LAST: OnceLock<Mutex<Option<(std::time::Instant, String)>>> = OnceLock::new();
    LAST.get_or_init(Default::default)
}

pub(crate) fn note_handoff(host: &str) {
    *last_handoff().lock_ok() = Some((std::time::Instant::now(), bare(host)));
}

/// The site to offer "looks wrong? use the compatibility tab" for, for a short while after a hand-over.
pub(crate) fn handoff_notice(page_host: &str) -> Option<String> {
    let last = last_handoff().lock_ok();
    let (at, host) = last.as_ref()?;
    (at.elapsed().as_secs() < 20 && *host == bare(page_host)).then(|| host.clone())
}

pub(crate) fn clear_handoff_notice() {
    *last_handoff().lock_ok() = None;
}

pub(crate) fn esc(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// What the tab itself holds: shown only when the system engine is not running (it is drawn
/// over this page when it is).
pub(crate) fn placeholder_html(url: &Url) -> String {
    let (title, shown) = match target(url) {
        Some(t) => (t.host_str().unwrap_or("page").to_string(), t.to_string()),
        None => ("compatibility tab".to_string(), String::new()),
    };
    let note = if usable() {
        "This page is shown by your system's web engine."
    } else {
        "The system web engine is not available here (on Windows it is WebView2, on Linux WebKitGTK in an X11 session), so this page cannot be shown. Copy the address and open it in another browser."
    };
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>{}</title>\
<body style=\"font-family:system-ui,sans-serif;margin:3em;max-width:40em;line-height:1.5\">\
<h2>Compatibility tab</h2><p>{}</p><p style=\"word-break:break-all\"><code>{}</code></p></body>",
        esc(&title),
        note,
        esc(&shown)
    )
}

/// Memory of the system engine's helper processes in MB, from `tasklist` CSV lines such as
/// `"msedgewebview2.exe","1234","Console","1","123,456 K"`.
pub(crate) fn parse_tasklist_mb(text: &str) -> u64 {
    text.lines()
        .filter(|line| line.to_ascii_lowercase().contains("msedgewebview2"))
        .filter_map(|line| line.rsplit("\",\"").next())
        .map(|field| field.chars().filter(char::is_ascii_digit).collect::<String>())
        .filter_map(|digits| digits.parse::<u64>().ok())
        .sum::<u64>()
        / 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn wraps_and_unwraps_pages() {
        let page = url("https://isdown.app/integrations/x?a=1&b=2");
        let wrapped = wrap(&page).unwrap();
        assert_eq!(wrapped.scheme(), "servo");
        assert_eq!(target(&wrapped), Some(page.clone()));
        assert_eq!(display(wrapped.as_str()), page.as_str());
        assert_eq!(display("https://example.com/"), "https://example.com/");
        assert!(wrap(&url("servo:newtab")).is_none() && wrap(&url("file:///c:/a")).is_none());
        assert!(target(&url("servo:compat?url=javascript%3Aalert(1)")).is_none());
        assert!(target(&url("servo:compat")).is_none() && target(&url("servo:newtab")).is_none());
    }

    #[test]
    fn knows_check_pages() {
        for title in ["Just a moment...", "Attention Required! | Cloudflare", "Performing security verification", "  checking your browser"] {
            assert!(is_challenge_title(title), "{title}");
        }
        assert!(!is_challenge_title("Moment of truth") && !is_challenge_title("Example Domain") && !is_challenge_title(""));
    }

    #[test]
    fn site_list_is_clean_and_matches_subdomains() {
        let sites = clean_sites(vec![" WWW.IsDown.app ".into(), "isdown.app".into(), "".into(), "bad host!".into(), "claude.ai".into()]);
        assert_eq!(sites, vec!["isdown.app".to_string(), "claude.ai".to_string()]);
        assert!(site_listed(&sites, "isdown.app") && site_listed(&sites, "www.isdown.app") && site_listed(&sites, "a.claude.ai"));
        assert!(!site_listed(&sites, "notisdown.app") && !site_listed(&sites, "isdown.app.evil.example"));
    }

    #[test]
    fn pasted_addresses_become_site_names() {
        assert_eq!(normalize_site(" https://www.Claude.ai/chat/1?x=2#y "), "claude.ai");
        assert_eq!(normalize_site("http://user:pw@isdown.app:8080/path"), "isdown.app");
        assert_eq!(normalize_site("example.com"), "example.com");
        assert_eq!(normalize_site("www.example.com/"), "example.com");
        assert_eq!(normalize_site("https://"), "");
        assert!(site_listed(&["example.com".to_string()], "Example.com."), "a trailing dot is the same host");
        assert_eq!(clean_sites(vec!["https://a.example/x".into(), "A.example".into(), "bad host!".into()]), vec!["a.example".to_string()]);
    }

    #[test]
    fn check_after_hand_over_fails_only_when_soon() {
        mark_handed_off("www.soon.example");
        assert_eq!(check_returned("soon.example"), Some(true));
        assert_eq!(check_returned("soon.example"), None, "counted once");
        if let Some(old) = Instant::now().checked_sub(FAIL_WINDOW + Duration::from_secs(1)) {
            handed().lock_ok().insert("late.example".into(), old);
            assert_eq!(check_returned("late.example"), Some(false));
            assert!(!handed_off("late.example"));
        }
    }

    #[test]
    fn history_skips_handed_over_compatibility_tabs() {
        let page = url("https://skip.example/a?x=1");
        let list = vec![url("https://start.example/"), url("https://skip.example/a"), wrap(&page).unwrap(), page.clone()];
        assert_eq!(history_skip("t1", &list, 3), None, "first report");
        assert_eq!(history_skip("t1", &list, 2), None, "not handed over");
        mark_handed_off("skip.example");
        history_skip("t1", &list, 3);
        assert_eq!(history_skip("t1", &list, 2), Some(-2), "Back: past the tab and the check page");
        history_skip("t1", &list, 1);
        assert_eq!(history_skip("t1", &list, 2), Some(1), "Forward: on to the page");
        assert_eq!(history_skip("t1", &list[..3], 2), None, "a fresh compatibility tab (nothing after it)");
        history_skip("t2", &list[1..], 2);
        assert_eq!(history_skip("t2", &list[1..], 1), Some(1), "nothing to go back to but the check page");
        mark_stay("skip.example");
    }

    #[test]
    fn effective_mode_follows_the_hand_over() {
        let mut settings = crate::pteron::Settings::default();
        settings.compat_tab_mode = "ask".into();
        assert_eq!(effective_mode(&settings), "ask");
        settings.compat_tab_handoff = true;
        assert_eq!(effective_mode(&settings), "auto");
    }

    #[test]
    fn switches_once_per_tab_and_page() {
        assert!(first_switch("tab1", "https://a.example/"));
        assert!(!first_switch("tab1", "https://a.example/"));
        assert!(first_switch("tab2", "https://a.example/"));
    }

    #[test]
    fn redirect_follows_the_list_and_mode() {
        if !cfg!(pteron_wry) {
            return;
        }
        let mut settings = crate::pteron::Settings::default();
        settings.compat_tab_sites = vec!["isdown.app".into()];
        let page = url("https://isdown.app/x");
        assert_eq!(redirect(&settings, &page), wrap(&page));
        assert!(redirect(&settings, &url("https://example.com/")).is_none());
        settings.compat_tab_mode = "off".into();
        assert!(redirect(&settings, &page).is_none());
        let claude = url("https://claude.ai/new");
        assert_eq!(redirect(&settings, &claude), wrap(&claude), "the keep list works in every mode");
        settings.compat_tab_mode = "auto".into();
        assert_eq!(redirect(&settings, &page), wrap(&page));
        settings.compat_tab_handoff = true;
        assert!(redirect(&settings, &page).is_none(), "with the hand-over on the other list is off");
        settings.compat_tab_keep_sites = vec!["isdown.app".into()];
        assert_eq!(redirect(&settings, &page), wrap(&page));
    }

    #[test]
    fn hand_over_state_is_per_site_and_failure_sticks() {
        assert!(!handed_off("a.example") && !stays("a.example"));
        mark_handed_off("WWW.A.example");
        assert!(handed_off("a.example") && !stays("a.example"));
        mark_stay("a.example");
        assert!(!handed_off("a.example") && stays("www.a.example"));
        assert!(!stays("b.example"));
    }

    #[test]
    fn handoff_notice_is_for_that_site_only() {
        note_handoff("WWW.Notice.example");
        assert_eq!(handoff_notice("notice.example").as_deref(), Some("notice.example"));
        assert!(handoff_notice("other.example").is_none());
        clear_handoff_notice();
        assert!(handoff_notice("notice.example").is_none());
    }

    #[test]
    fn placeholder_is_escaped() {
        let html = placeholder_html(&wrap(&url("https://a.example/?q=<b>\"x\"")).unwrap());
        assert!(!html.contains("<b>") && html.contains("a.example"));
    }

    #[test]
    fn adds_up_helper_process_memory() {
        let text = "\"msedgewebview2.exe\",\"10\",\"Console\",\"1\",\"102,400 K\"\n\"other.exe\",\"11\",\"Console\",\"1\",\"9,999 K\"\n\"msedgewebview2.exe\",\"12\",\"Console\",\"1\",\"51,200 K\"\n";
        assert_eq!(parse_tasklist_mb(text), 150);
        assert_eq!(parse_tasklist_mb("INFO: No tasks are running"), 0);
    }
}
