#![allow(dead_code, unused)]
// Only the modules that do not need Servo itself.
#[path = "../../ports/servoshell/innertube.rs"]
mod innertube;
#[path = "../../ports/servoshell/streamproxy.rs"]
mod streamproxy;
#[path = "../../ports/servoshell/mp4mux.rs"]
mod mp4mux;
#[path = "../../ports/servoshell/crashlog.rs"]
mod crashlog;
#[path = "../../ports/servoshell/youtube.rs"]
mod youtube;
#[path = "../../ports/servoshell/downloads.rs"]
mod downloads;
#[path = "../../ports/servoshell/library.rs"]
mod library;
#[path = "../../ports/servoshell/sync.rs"]
mod sync;
#[path = "../../ports/servoshell/reader.rs"]
mod reader;
#[path = "../../ports/servoshell/secure.rs"]
mod secure;
#[path = "../../ports/servoshell/mp3.rs"]
mod mp3;
#[path = "../../ports/servoshell/sitecheck.rs"]
mod sitecheck;
#[path = "../../ports/servoshell/compat.rs"]
mod compat;
#[path = "../../ports/servoshell/memsaver.rs"]
mod memsaver;
#[path = "../../ports/servoshell/filters.rs"]
mod filters;
#[path = "../../ports/servoshell/privacy.rs"]
mod privacy;
#[path = "../../ports/servoshell/locks.rs"]
mod locks;
#[path = "../../ports/servoshell/util.rs"]
mod util;
#[path = "../../ports/servoshell/compat_view.rs"]
mod compat_view;
#[path = "../../ports/servoshell/signing.rs"]
mod signing;
#[path = "../../ports/servoshell/multisearch.rs"]
mod multisearch;
#[path = "../../ports/servoshell/passwords.rs"]
mod passwords;
#[path = "../../ports/servoshell/adblock.rs"]
mod adblock;
#[path = "../../ports/servoshell/appearance.rs"]
mod appearance;
#[path = "../../ports/servoshell/find.rs"]
mod find;
#[path = "../../ports/servoshell/pteron.rs"]
mod pteron;
#[path = "../../ports/servoshell/updater.rs"]
mod updater;
#[path = "../../ports/servoshell/report.rs"]
mod report;
#[path = "../../ports/servoshell/music.rs"]
mod music;
#[path = "../../ports/servoshell/desktop/sidebar.rs"]
mod sidebar;
#[path = "../../ports/servoshell/desktop/settings_page.rs"]
mod settings_page;

mod compat_host {
    pub(crate) fn alive_count() -> usize { 0 }
    pub(crate) fn engine_mb() -> Option<u64> { None }
}
mod window {
    pub(crate) fn trim_process_memory() {}
}
fn main() {}
