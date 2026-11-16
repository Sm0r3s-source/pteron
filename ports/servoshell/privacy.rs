/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pteron: privacy helpers. Removes tracking parts from addresses, and tells pages the user
//! does not want to be tracked (the network side, the `DNT` and `Sec-GPC` headers, is a Servo
//! preference set at start-up; this is the matching `navigator` side).

use url::Url;

/// Query parts that only identify a click or a campaign, never what the page shows.
fn is_tracking(name: &str) -> bool {
    const NAMES: &[&str] = &[
        "fbclid", "gclid", "dclid", "gbraid", "wbraid", "msclkid", "yclid", "twclid", "igshid", "mc_eid",
        "mc_cid", "_hsenc", "_hsmi", "mkt_tok", "oly_enc_id", "oly_anon_id", "vero_id", "__s",
    ];
    let name = name.to_ascii_lowercase();
    name.starts_with("utm_") || NAMES.contains(&name.as_str())
}

/// The address without tracking parts, or `None` when there was nothing to remove.
/// Only web addresses are touched; the other query parts are kept exactly as they were.
pub(crate) fn strip_tracking(url: &Url) -> Option<Url> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let query = url.query()?;
    let kept: Vec<&str> = query
        .split('&')
        .filter(|part| !part.is_empty() && !is_tracking(part.split('=').next().unwrap_or("")))
        .collect();
    if kept.len() == query.split('&').count() {
        return None;
    }
    let mut clean = url.clone();
    clean.set_query((!kept.is_empty()).then(|| kept.join("&")).as_deref());
    Some(clean)
}

/// Makes `navigator.doNotTrack` and `navigator.globalPrivacyControl` say "do not track".
pub(crate) const SCRIPT: &str = "(function(){try{var n=Navigator.prototype;\
Object.defineProperty(n,'doNotTrack',{get:function(){return '1';},configurable:true});\
Object.defineProperty(n,'globalPrivacyControl',{get:function(){return true;},configurable:true});}catch(e){}})();";

/// Pteron: whether `url` points into the user's own computer or home network (router, printer,
/// local servers): loopback, private and link-local addresses, 0.0.0.0, and names like
/// `localhost` or `printer.local`.
pub(crate) fn is_private_address(url: &Url) -> bool {
    use std::net::{IpAddr, Ipv4Addr};
    let v4 = |ip: Ipv4Addr| {
        ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() ||
            (ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
    };
    let ip = |ip: IpAddr| match ip {
        IpAddr::V4(ip) => v4(ip),
        IpAddr::V6(ip) => {
            ip.is_loopback() ||
                ip.is_unspecified() ||
                (ip.segments()[0] & 0xfe00) == 0xfc00 ||
                (ip.segments()[0] & 0xffc0) == 0xfe80 ||
                ip.to_ipv4_mapped().is_some_and(v4)
        },
    };
    match url.host() {
        Some(url::Host::Ipv4(addr)) => ip(IpAddr::V4(addr)),
        Some(url::Host::Ipv6(addr)) => ip(IpAddr::V6(addr)),
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" ||
                [".localhost", ".local", ".internal", ".home.arpa"].iter().any(|end| name.ends_with(end))
        },
        None => false,
    }
}

/// Pteron: a site on the internet may not make the browser send requests into your own computer
/// or home network (a page could otherwise change router settings or talk to programs that
/// only listen locally). Pages that are themselves local, and Pteron's own pages, are not limited.
pub(crate) fn blocks_private_request(target: &Url, page: Option<&Url>) -> bool {
    let Some(page) = page else { return false };
    matches!(page.scheme(), "http" | "https") &&
        page.host().is_some() &&
        !is_private_address(page) &&
        matches!(target.scheme(), "http" | "https" | "ws" | "wss") &&
        is_private_address(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(url: &str) -> Option<String> {
        strip_tracking(&Url::parse(url).unwrap()).map(|u| u.to_string())
    }

    #[test]
    fn removes_only_tracking_parts() {
        assert_eq!(
            clean("https://a.example/p?id=7&utm_source=x&fbclid=abc&q=a%20b").as_deref(),
            Some("https://a.example/p?id=7&q=a%20b")
        );
        assert_eq!(clean("https://a.example/p?UTM_Medium=m&gclid=1").as_deref(), Some("https://a.example/p"));
        assert_eq!(clean("https://a.example/p?x=1#top&utm_x=1"), None);
        assert_eq!(clean("https://a.example/p?x=1#frag").as_deref(), None);
        assert_eq!(clean("https://a.example/p?utm_source=x#frag").as_deref(), Some("https://a.example/p#frag"));
    }

    #[test]
    fn leaves_everything_else_alone() {
        assert_eq!(clean("https://a.example/"), None);
        assert_eq!(clean("https://a.example/?q=utm_source"), None);
        assert_eq!(clean("https://a.example/?utm_nope"), Some("https://a.example/".to_string()));
        assert_eq!(clean("servo:newtab?utm_source=x"), None);
        assert_eq!(clean("file:///C:/a.html?fbclid=1"), None);
        // Cleaning twice changes nothing more.
        assert_eq!(clean("https://a.example/?a=1"), None);
    }

    #[test]
    fn internet_pages_cannot_reach_the_home_network() {
        let u = |t: &str| Url::parse(t).unwrap();
        let page = u("https://news.example/");
        for inside in ["http://192.168.1.1/apply.cgi", "http://10.0.0.5/", "http://127.0.0.1:8080/", "http://0.0.0.0:631/",
            "http://[::1]/", "http://[fd00::1]/", "http://printer.local/", "http://localhost:3000/", "ws://127.0.0.1:9222/"] {
            assert!(blocks_private_request(&u(inside), Some(&page)), "{inside}");
        }
        assert!(!blocks_private_request(&u("https://cdn.example/a.js"), Some(&page)));
        assert!(!blocks_private_request(&u("http://8.8.8.8/"), Some(&page)));
        // A local page (a router's own page, Pteron's helper pages) may.
        assert!(!blocks_private_request(&u("http://192.168.1.1/x"), Some(&u("http://192.168.1.1/"))));
        assert!(!blocks_private_request(&u("http://127.0.0.1:5000/v"), Some(&u("file:///home/u/page.html"))));
        assert!(!blocks_private_request(&u("http://127.0.0.1:5000/v"), None));
    }
}
