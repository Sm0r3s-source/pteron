/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pteron: how the browser is laid out and shaped (Pteron's sidebar, or top tabs like other
//! browsers), interface size and buttons, dark mode and your own styles for web pages, and the
//! shortcuts on the new tab page. Plain data, so it is tested without a window.

use serde::{Deserialize, Serialize};

use crate::compat_view::{clean_sites, site_listed};

/// Browser layouts: id, name, description, matching colour theme ("" keeps the theme), corners.
pub(crate) const LAYOUTS: [(&str, &str, &str, &str, u32); 5] = [
    ("sidebar", "Pteron", "Tabs in a sidebar that slides in from the edge of the window.", "", 8),
    ("chrome", "Chrome style", "Tabs on top, joined to the toolbar. Menu at the right.", "daylight", 8),
    ("firefox", "Firefox style", "Floating tabs on top, address bar in the middle.", "mist", 6),
    ("opera", "Opera style", "Tabs on top and a bar of shortcuts down the left side.", "nightred", 6),
    ("gx", "Opera GX style", "Dark gaming look: neon highlights and sharp corners.", "neon", 2),
];

/// Buttons that can be hidden: id, name.
pub(crate) const BUTTONS: [(&str, &str); 10] = [
    ("back", "Back"),
    ("forward", "Forward"),
    ("reload", "Reload"),
    ("home", "Home (new tab page)"),
    ("bookmark", "Bookmark star"),
    ("reader", "Reading mode"),
    ("dark", "Dark mode for this site (when dark mode is on)"),
    ("copy", "Copy link"),
    ("newwindow", "New window"),
    ("experimental", "Experimental features"),
];

/// Most shortcuts kept for the new tab page, and most site styles.
const MAX_SHORTCUTS: usize = 24;
const MAX_STYLES: usize = 100;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SiteStyle {
    /// A host name ("example.org", also covers its subdomains) or "*" for every site.
    pub site: String,
    pub css: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Shortcut {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Look {
    /// An id from [`LAYOUTS`].
    pub layout: String,
    pub sidebar_right: bool,
    pub sidebar_width: u32,
    /// Interface size in percent.
    pub scale: u32,
    /// "compact", "normal" or "roomy".
    pub density: String,
    /// Corner radius of buttons and boxes.
    pub roundness: u32,
    /// Ids from [`BUTTONS`] that are not shown.
    pub hidden_buttons: Vec<String>,
    pub tab_icons: bool,
    /// Close buttons on every tab, not only the one under the mouse.
    pub close_always: bool,
    /// A row of bookmarks under the toolbar (top-tab layouts).
    pub bookmarks_bar: bool,
    /// The custom theme's other colours ("" = worked out from its background).
    pub custom_surface: String,
    pub custom_text: String,
    pub custom_muted: String,
    /// Dark mode for web pages, and the sites it leaves alone.
    pub dark_pages: bool,
    pub dark_skip: Vec<String>,
    pub site_styles: Vec<SiteStyle>,
    /// The user's own tiles on the new tab page.
    pub shortcuts: Vec<Shortcut>,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            layout: "sidebar".into(),
            sidebar_right: false,
            sidebar_width: 260,
            scale: 100,
            density: "normal".into(),
            roundness: 8,
            hidden_buttons: vec!["home".into()],
            tab_icons: true,
            close_always: false,
            bookmarks_bar: true,
            custom_surface: String::new(),
            custom_text: String::new(),
            custom_muted: String::new(),
            dark_pages: false,
            dark_skip: Vec::new(),
            site_styles: Vec::new(),
            shortcuts: Vec::new(),
        }
    }
}

impl Look {
    /// Read from a settings file, where anything may be missing or out of range.
    pub(crate) fn from_json(value: &serde_json::Value) -> Self {
        serde_json::from_value::<Look>(value.clone()).unwrap_or_default().cleaned()
    }

    pub(crate) fn cleaned(mut self) -> Self {
        let default = Look::default();
        if !LAYOUTS.iter().any(|(id, ..)| *id == self.layout) {
            self.layout = default.layout;
        }
        if !matches!(self.density.as_str(), "compact" | "normal" | "roomy") {
            self.density = default.density;
        }
        self.sidebar_width = self.sidebar_width.clamp(200, 420);
        self.scale = self.scale.clamp(75, 175);
        self.roundness = self.roundness.min(16);
        self.hidden_buttons.retain(|id| BUTTONS.iter().any(|(known, _)| known == id));
        for color in [&mut self.custom_surface, &mut self.custom_text, &mut self.custom_muted] {
            if crate::appearance::parse_hex(color).is_none() {
                color.clear();
            }
        }
        self.dark_skip = clean_sites(std::mem::take(&mut self.dark_skip));
        self.site_styles.retain_mut(|style| {
            style.site = clean_style_site(&style.site);
            !style.site.is_empty()
        });
        self.site_styles.truncate(MAX_STYLES);
        self.shortcuts.retain(|s| shortcut_url(&s.url).is_some());
        self.shortcuts.truncate(MAX_SHORTCUTS);
        self
    }

    pub(crate) fn shows(&self, button: &str) -> bool {
        !self.hidden_buttons.iter().any(|id| id == button)
    }

    pub(crate) fn top_tabs(&self) -> bool {
        self.layout != "sidebar"
    }

    /// Height of a tab or list row.
    pub(crate) fn row_height(&self) -> f32 {
        match self.density.as_str() {
            "compact" => 24.0,
            "roomy" => 34.0,
            _ => 28.0,
        }
    }

    /// Whether dark mode applies to pages of `host`.
    pub(crate) fn dark_for(&self, host: &str) -> bool {
        self.dark_pages && !site_listed(&self.dark_skip, host)
    }

    /// Switch dark mode off for `host` (or back on).
    pub(crate) fn toggle_dark_site(&mut self, host: &str) {
        let site = host.trim_start_matches("www.").to_ascii_lowercase();
        if site_listed(&self.dark_skip, &site) {
            self.dark_skip.retain(|s| !(site == *s || site.ends_with(&format!(".{s}"))));
        } else {
            self.dark_skip.push(site);
            self.dark_skip = clean_sites(std::mem::take(&mut self.dark_skip));
        }
    }

    /// Add a new tab shortcut; false when the address is not a web page.
    pub(crate) fn add_shortcut(&mut self, name: &str, url: &str) -> bool {
        let Some(url) = shortcut_url(url) else {
            return false;
        };
        self.shortcuts.retain(|s| s.url != url);
        let name = name.trim().chars().take(40).collect::<String>();
        self.shortcuts.push(Shortcut { name, url });
        self.shortcuts.truncate(MAX_SHORTCUTS);
        true
    }
}

/// A site for a style: a host name, or "*" for all sites.
pub(crate) fn clean_style_site(text: &str) -> String {
    if text.trim() == "*" {
        return "*".into();
    }
    clean_sites(vec![text.to_string()]).pop().unwrap_or_default()
}

/// The address a shortcut opens: http(s) only; "example.org" becomes "https://example.org/".
pub(crate) fn shortcut_url(text: &str) -> Option<String> {
    let text = text.trim();
    let with_scheme = if text.contains("://") { text.to_string() } else { format!("https://{text}") };
    let url = url::Url::parse(&with_scheme).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some_and(|h| h.contains('.') || h == "localhost"))
        .then(|| url.to_string())
}

/// The style that darkens a light page: the page is turned to negative, and pictures and videos
/// back again so they keep their colours. (The page's backdrop is not part of what the engine
/// turns to negative, so it is given the dark colour directly.)
const DARK_CSS: &str = "html{filter:invert(.9) hue-rotate(180deg)!important;background:#191919!important}\
img,video,picture,canvas,iframe,embed,object,svg image,[style*='background-image']{filter:invert(1) hue-rotate(180deg)!important}";

/// Runs in every page: adds the user's styles for the site and, with dark mode, darkens pages
/// that are still light once their own styles are in (pages with their own dark look keep it,
/// because they are told the user prefers dark).
const PAGE_SCRIPT: &str = r#"(function(){try{
var c=__CONFIG__,d=document,h=location.hostname.toLowerCase().replace(/^www\./,'');
function m(s){return s==='*'||h===s||h.slice(-s.length-1)==='.'+s}
function add(id,css){var e=d.getElementById(id)||d.createElement('style');e.id=id;e.textContent=css;(d.head||d.documentElement).appendChild(e)}
var own=c.styles.filter(function(s){return m(s[0])}).map(function(s){return s[1]}).join('\n');
if(own)add('pteron-site-style',own);
if(!c.dark||c.skip.some(m)||!/^(https?|file):$/.test(location.protocol))return;
function lum(x){var v=(x||'').match(/[\d.]+/g);if(!v||(v.length>3&&+v[3]===0))return -1;return(.2126*v[0]+.7152*v[1]+.0722*v[2])/255}
function check(){if(d.getElementById('pteron-dark')||!d.body)return;var b=lum(getComputedStyle(d.body).backgroundColor),r=lum(getComputedStyle(d.documentElement).backgroundColor);if((b>=0?b:(r>=0?r:1))>.6)add('pteron-dark',c.css)}
if(d.readyState!=='loading')check();d.addEventListener('DOMContentLoaded',check);addEventListener('load',check);
}catch(e){}})();"#;

/// The script for dark mode and site styles, or `None` when neither is in use.
pub(crate) fn page_script(look: &Look) -> Option<String> {
    if !look.dark_pages && look.site_styles.is_empty() {
        return None;
    }
    let config = serde_json::json!({
        "dark": look.dark_pages,
        "skip": look.dark_skip,
        "css": DARK_CSS,
        "styles": look
            .site_styles
            .iter()
            .map(|s| [clean_style_site(&s.site), s.css.clone()])
            .filter(|[site, _]| !site.is_empty())
            .collect::<Vec<_>>(),
    });
    Some(PAGE_SCRIPT.replace("__CONFIG__", &config.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_values_are_put_right() {
        let look = Look::from_json(&serde_json::json!({
            "layout": "netscape", "scale": 900, "sidebar_width": 5, "roundness": 99, "density": "huge",
            "hidden_buttons": ["back", "teleport"], "custom_text": "nope", "custom_surface": "#102030",
            "dark_skip": ["https://www.Example.org/path", "bad site!"],
            "site_styles": [{"site": "*", "css": "a{}"}, {"site": "", "css": "b{}"}, {"site": "news.example.com/x", "css": "c{}"}],
            "shortcuts": [{"name": "A", "url": "https://a.org/"}, {"name": "B", "url": "javascript:alert(1)"}],
        }));
        assert_eq!((look.layout.as_str(), look.scale, look.sidebar_width, look.roundness), ("sidebar", 175, 200, 16));
        assert_eq!(look.density, "normal");
        assert_eq!(look.hidden_buttons, vec!["back".to_string()]);
        assert_eq!((look.custom_text.as_str(), look.custom_surface.as_str()), ("", "#102030"));
        assert_eq!(look.dark_skip, vec!["example.org".to_string()]);
        let sites: Vec<&str> = look.site_styles.iter().map(|s| s.site.as_str()).collect();
        assert_eq!(sites, vec!["*", "news.example.com"]);
        assert_eq!(look.shortcuts.len(), 1);
        assert_eq!(Look::from_json(&serde_json::json!("garbage")), Look::default());
        assert_eq!(Look::from_json(&serde_json::json!({})), Look::default());
    }

    #[test]
    fn dark_mode_can_be_switched_off_per_site() {
        let mut look = Look { dark_pages: true, ..Look::default() };
        assert!(look.dark_for("mail.example.org"));
        look.toggle_dark_site("www.example.org");
        assert!(!look.dark_for("mail.example.org") && !look.dark_for("example.org"));
        assert!(look.dark_for("example.com"));
        look.toggle_dark_site("example.org");
        assert!(look.dark_for("example.org") && look.dark_skip.is_empty());
        look.dark_pages = false;
        assert!(!look.dark_for("example.com"));
    }

    #[test]
    fn shortcuts_take_web_addresses_only() {
        let mut look = Look::default();
        assert!(look.add_shortcut(" News ", "news.example.org"));
        assert_eq!(look.shortcuts[0], Shortcut { name: "News".into(), url: "https://news.example.org/".into() });
        assert!(!look.add_shortcut("x", "file:///etc/passwd"));
        assert!(!look.add_shortcut("x", "javascript:alert(1)"));
        assert!(!look.add_shortcut("x", "nodots"));
        assert!(look.add_shortcut("Again", "https://news.example.org/"));
        assert_eq!(look.shortcuts.len(), 1);
        assert_eq!(look.shortcuts[0].name, "Again");
    }

    #[test]
    fn page_script_only_when_needed() {
        let mut look = Look::default();
        assert!(page_script(&look).is_none());
        look.site_styles.push(SiteStyle { site: "example.org".into(), css: "body{color:red}</script>".into() });
        let script = page_script(&look).unwrap();
        assert!(script.contains("\"dark\":false") && script.contains("body{color:red}"));
        assert!(!script.contains("__CONFIG__"));
        look.dark_pages = true;
        assert!(page_script(&look).unwrap().contains("invert(.9)"));
    }

    #[test]
    fn every_layout_has_a_known_theme() {
        for (id, _, _, theme, _) in LAYOUTS {
            assert!(theme.is_empty() || crate::appearance::THEMES.iter().any(|(t, ..)| *t == theme), "{id}");
        }
    }
}
