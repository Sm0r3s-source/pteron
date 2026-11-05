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
}
