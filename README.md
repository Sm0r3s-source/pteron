# Pteron

A light web browser for Windows, built on the [Servo](https://servo.org) engine (v0.7.0) in Rust.
Sidebar tabs, tabs that sleep to save memory, its own YouTube player and music app, a password
manager, an ad blocker, and a Memory saver mode for older computers.

Pteron is an early test version. Servo does not support every web feature yet, so some sites look
wrong or do not work (see [Known problems](#known-problems)).

## Features

**Browsing**
- Sidebar with tabs, pinned tabs, bookmarks (one folder level), history and downloads
- Tabs sleep after a while and wake when clicked; session restore brings tabs back asleep
- Find in page, reading mode (the **Aa** button), popup blocking
- Address bar shortcuts: `g`, `ddg`, `yt`, `w`, `gh`, `maps`, `r`, and `i` for pictures
  (for example `yt lofi`, `i red car`)
- Themes, accent colours and a custom new tab page (background, tiles)
- Search engines: DuckDuckGo, Google, Bing, Brave, Startpage or your own SearXNG; optionally
  search several engines at once (one tab per engine, or one merged results page)

**Plume (YouTube) and Plume Music**
- YouTube search, channels and videos open in simple Pteron pages with a built-in player (up to
  1080p60, H.264), because Servo has no Media Source Extensions yet
- Plume Music for listening, with queues, playlists and liked songs
- Download videos as .mp4 and songs as .mp3 (converted inside Pteron)

**Privacy and security**
- Ad and tracker blocking: a built-in list of well-known networks, plus optional full EasyList and EasyPrivacy lists (uses 15 to 30 MB more memory)
- Tracking parts (`utm_*`, `fbclid`, `gclid`, ...) removed from links
- Do Not Track and Global Privacy Control signals (after a restart)
- https first: plain http sites are checked for an https version; otherwise a warning page appears
- Saved passwords on ordinary login forms, locked with your Windows account, kept only on this
  computer, never synced and never included in problem reports
- Clear history and cache on exit (optional); sync never carries security switches
- Updates must carry a SHA-256 file and a signature from the author's key, or they are refused

**Low memory**
- Settings, Tabs, **Memory saver**: Normal, Low memory and Lowest memory levels trade speed for
  less RAM (smaller video buffer, earlier tab sleep, fewer helper threads, smaller cache)
- A memory log is written next to the crash log
- Typical use on the author's computers: around 100 to 140 MB with a blank tab, more with
  YouTube open. Measure on your own PC; the numbers depend on the machine

**Web compatibility**
- Settings, **Web compatibility**: switch on newer web features one by one (IndexedDB, observers,
  fonts, canvas, storage, container queries). They are off by default because they can make
  some sites worse
- After a crash, Pteron starts in safe mode with those features off
- **Check this page** (tab context menu) writes a small report about what a page is missing

**Other**
- Sync of bookmarks and settings through a folder you choose
- Self-update from Settings, About
- Crash log, problem report file, and a notice with **Open in default browser** for sites that sit
  behind Cloudflare's check (Pteron cannot pass it)

## Installing and updating

Releases are published on the [Releases page](https://github.com/Sm0r3s-source/pteron/releases)
as `Pteron-Setup.exe` and a zip. Pteron updates itself from **Settings, About**; nothing is
installed without your say-so. Windows may warn about the installer because it is not signed yet.

## Building

Pteron is a set of changes on top of a Servo v0.7.0 checkout, not a fork with its own history.
The update scripts copy the changed files into the checkout and build it.

You need a normal Servo build setup on Windows:
- Rust (the toolchain Servo pins)
- Visual Studio Build Tools with the "Desktop development with C++" workload
- LLVM (`winget install LLVM.LLVM`), so that `lld-link.exe` is on your PATH
- The other prerequisites listed in the [Servo book](https://book.servo.org)

Then, from the folder with an update:

```
powershell -ExecutionPolicy Bypass -File .\pteron_updateNN.ps1 -Publish
```

`-NoBuild` copies the files without building; `-Publish` builds, packages and publishes a release.
A fully reproducible build (a real Servo fork with a lockfile, built in CI) is planned.

## Where things are

New files are in `ports/servoshell/`. Some of the main ones:

| File | What it does |
|---|---|
| `desktop/sidebar.rs`, `desktop/settings_page.rs` | The sidebar and the Settings page (egui) |
| `youtube.rs`, `innertube.rs`, `streamproxy.rs`, `mp4mux.rs` | Plume: YouTube pages, API, local video helper, MP4 muxer |
| `music.rs`, `downloads.rs`, `mp3.rs` | Plume Music, downloads, MP3 conversion |
| `adblock.rs`, `privacy.rs`, `secure.rs`, `passwords.rs` | Blocking, tracking removal, https, passwords |
| `sync.rs`, `updater.rs`, `library.rs` | Sync, self-update, bookmarks and history |
| `compat.rs`, `sitecheck.rs`, `crashlog.rs`, `report.rs` | Compatibility, page check, crash and problem reports |
| `memsaver.rs`, `multisearch.rs`, `util.rs`, `locks.rs` | Memory saver, multi-engine search, shared helpers |

Pteron also makes small changes in Servo's own files (`components/config/prefs.rs`,
`components/net/http_loader.rs`, `components/servo/servo.rs`, the GStreamer media backend, and the
media controls).

## Known problems

- Protected (DRM) media does not work: Spotify, Netflix, Disney+ and similar
- Strict anti-bot checks, including Cloudflare's "Just a moment" page, cannot be passed
- Some modern pages look wrong, stay blank or load slowly; the engine does not support every web feature
- Windows only for now; no extensions and no private window
- Videos play H.264 up to 1080p60; VP9, AV1 and 4K are not supported

The same list, kept up to date, is in the browser at `servo:known-issues`.

## Reporting problems

Open the sidebar, then **Settings, About, Report a problem**. It saves a small file on your
Desktop with the details (no passwords). Attach it to an issue.

## License

Pteron is released under the [Mozilla Public License 2.0](https://www.mozilla.org/MPL/2.0/), the
same as Servo.
