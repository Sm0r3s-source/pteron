//! The system web engine (WebView2 on Windows, WebKit on macOS) drawn inside Pteron's window
//! over the page area, one view per compatibility tab. Views are created when a compatibility
//! tab is first shown, hidden (and told to use less memory) when it is not, and dropped after a
//! while hidden; the address is kept, so the tab reloads when it is clicked again.

use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use servo::WebViewId;

// The engine library is `wry`: WebView2 on Windows, WebKit on macOS, and WebKitGTK on Linux (X11) when
// built with the `compat_linux` feature.

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::locks::LockExt;

/// Hidden views are dropped after this long (shorter with a Memory saver level).
const KEEP_HIDDEN_SECS: u64 = 120;

struct Entry {
    #[cfg(pteron_wry)]
    view: Option<wry::WebView>,
    /// Where the tab was last seen (kept when the view is dropped).
    url: Arc<Mutex<String>>,
    hidden_since: Option<Instant>,
    /// The address the tab was opened with (`servo:compat?url=...`'s page). A tab that gets another
    /// compatibility address (typed, or Back/Forward between two) starts a new view.
    target: String,
    flags: Arc<Flags>,
    /// What the view was last told, so the engine is only called when something changes
    /// (this runs every frame): on screen, its area in pixels, low memory.
    shown: bool,
    bounds: Option<(i32, i32, u32, u32)>,
    low: bool,
    /// When the engine was last asked for its browser name.
    agent_asked: Option<Instant>,
}

/// What a view's page did, set by that view's own handlers and read when it is shown.
#[derive(Default)]
struct Flags {
    /// A browser check was seen and the page after it has not loaded yet.
    saw_check: AtomicBool,
    /// The check just passed: a short memory trim.
    trim_now: AtomicBool,
    /// A real page (not a browser check) loaded.
    page_ready: AtomicBool,
}

#[derive(Default)]
pub(crate) struct Host {
    entries: HashMap<WebViewId, Entry>,
    /// A memory trim is running until then (the engine is told "low memory" for a moment).
    trim_until: Option<Instant>,
    /// A text box of Pteron's own (the sidebar search) had the keyboard last frame.
    typing: bool,
    /// Tabs just handed over to Servo: no new view for them until Servo has loaded the page.
    released: HashMap<WebViewId, Instant>,
    /// A compatibility view was on screen last frame (to give the keyboard back when it goes away).
    was_shown: bool,
    handoff: Option<Handoff>,
    /// A view existed last time round (to notice when the last one is gone).
    had_view: bool,
    /// The last view closed then: the engine's helper processes are ended a few seconds later.
    closed_at: Option<Instant>,
    #[cfg(pteron_wry)]
    context: Option<wry::WebContext>,
}

/// Browser arguments: WebView2 shares its helper processes between views that use the same ones.
#[cfg(windows)]
fn browser_args() -> String {
    // Measured on a Cloudflare page: about 280 MB without the saving flags, about 170 MB with them,
    // so they are always on.
    let mut args = String::from(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
         --enable-low-end-device-mode --renderer-process-limit=2 --disable-background-networking",
    );
    if crate::pteron::settings().compat_tab_no_gpu {
        args.push_str(" --disable-gpu");
    }
    args
}

/// Linux: WebKitGTK draws and works through GTK's main loop, which nothing else runs, so Pteron
/// runs the waiting GTK events itself (a few times a second while a compatibility view is open).
#[cfg(all(target_os = "linux", pteron_wry))]
pub(crate) fn pump_gtk() {
    if alive_count() > 0 && gtk_ready() {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
    }
}

#[cfg(not(all(target_os = "linux", pteron_wry)))]
pub(crate) fn pump_gtk() {}

/// When the event loop has to wake up to run GTK's events (Linux, while a view is open).
pub(crate) fn gtk_wake() -> Option<Instant> {
    (cfg!(all(target_os = "linux", pteron_wry)) && alive_count() > 0).then(|| Instant::now() + Duration::from_millis(20))
}

/// GTK was started (once) on the thread that makes the views.
#[cfg(all(target_os = "linux", pteron_wry))]
fn gtk_ready() -> bool {
    static READY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *READY.get_or_init(|| gtk::init().is_ok())
}

/// Linux: Pteron starts in X11 mode (through XWayland on a Wayland desktop) because the
/// system web engine can only be placed inside an X11 window. `PTERON_WAYLAND=1` keeps Wayland
/// (the compatibility tab is then not available).
#[cfg(target_os = "linux")]
pub(crate) fn prefer_x11() -> bool {
    std::env::var_os("PTERON_WAYLAND").is_none_or(|v| v.is_empty() || v == "0") &&
        std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
}

/// Where the mouse is, as the page itself reports it: 0 unknown or outside, 1 over the page, 2 at its left edge.
static POINTER: AtomicU8 = AtomicU8::new(0);

/// Added to every page: tells Pteron where the mouse is, because egui cannot see the mouse over
/// a native view (the sidebar needs it to open and close).
#[cfg(pteron_wry)]
const POINTER_SCRIPT: &str = "(()=>{let s='';const p=v=>{if(v!==s){s=v;try{window.ipc.postMessage(v)}catch(e){}}};\
addEventListener('mousemove',e=>p(e.clientX<=5?'edge':'in'),{passive:true,capture:true});\
document.addEventListener('mouseleave',()=>p('out'));})();";

/// The engine's helper processes are being ended: no new view starts until that is done (it
/// would join the processes being ended).
static CLEANING: AtomicBool = AtomicBool::new(false);

/// What the system engine calls itself (read once, from the page); a hand-over needs Servo to
/// use the same name, because the pass cookie belongs to it.
static ENGINE_AGENT: Mutex<String> = Mutex::new(String::new());

/// The page title changed: the first real page after a browser check asks for a memory trim.
#[cfg(pteron_wry)]
fn title_changed(flags: &Flags, title: &str) {
    if crate::compat_view::is_challenge_title(title) {
        flags.saw_check.store(true, Relaxed);
    } else {
        flags.page_ready.store(true, Relaxed);
        if flags.saw_check.swap(false, Relaxed) {
            flags.trim_now.store(true, Relaxed);
        }
    }
    // egui does not see what happens inside the view: draw a frame to act on it.
    crate::window::repaint_after(Duration::ZERO);
}

/// Everything Servo needs to carry on where the system engine passed a browser check.
pub(crate) struct Handoff {
    pub id: WebViewId,
    pub page: url::Url,
    /// The pass and the other cookies of the site, as `Set-Cookie` text.
    pub cookies: Vec<String>,
    pub agent: String,
}

/// Servo's own browser name, from before the first hand-over.
static OWN_AGENT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Give Servo the cookies and the browser name (`apply` is the only place that needs the engine library).
#[cfg(pteron_wry)]
pub(crate) fn apply_handoff(servo: &servo::Servo, handoff: &Handoff) {
    for text in &handoff.cookies {
        if let Ok(cookie) = wry::cookie::Cookie::parse(text.clone()) {
            servo.site_data_manager().set_cookie_for_url(handoff.page.clone(), cookie, None);
        }
    }
    OWN_AGENT.get_or_init(|| servo::prefs::get().user_agent.clone());
    servo.set_preference("user_agent", servo::PrefValue::Str(handoff.agent.clone()));
}

#[cfg(not(pteron_wry))]
pub(crate) fn apply_handoff(_: &servo::Servo, _: &Handoff) {}

/// The hand-over was switched off: Servo goes back to its own browser name.
pub(crate) fn restore_agent(servo: &servo::Servo) {
    if let Some(agent) = OWN_AGENT.get() {
        servo.set_preference("user_agent", servo::PrefValue::Str(agent.clone()));
    }
    crate::compat_view::forget_handoffs();
}

pub(crate) fn pointer_in_page() -> bool {
    POINTER.load(Relaxed) == 1
}

pub(crate) fn pointer_at_edge() -> bool {
    POINTER.load(Relaxed) == 2
}

static ALIVE: AtomicUsize = AtomicUsize::new(0);

/// How many compatibility views exist (read by the memory log).
pub(crate) fn note_alive(count: usize) {
    ALIVE.store(count, Relaxed);
}

pub(crate) fn alive_count() -> usize {
    ALIVE.load(Relaxed)
}

impl Host {
    /// A finished hand-over, once: the tab goes back to Servo, which loads the page with the cookies.
    pub(crate) fn take_handoff(&mut self) -> Option<Handoff> {
        self.handoff.take()
    }

    /// Drop a tab's view now (it was handed over to Servo).
    pub(crate) fn release(&mut self, id: WebViewId) {
        self.entries.remove(&id);
        self.released.insert(id, Instant::now());
    }

    /// Something is waiting on a timer (a memory trim, or the engine's clean-up): redraw soon.
    pub(crate) fn trimming(&self) -> bool {
        self.trim_until.is_some() ||
            self.closed_at.is_some() ||
            CLEANING.load(Relaxed) ||
            !self.released.is_empty() ||
            self.entries.values().any(|e| e.hidden_since.is_some() && e.is_open())
    }

    /// Closing a view leaves the engine's helper processes running for a long time. Once the last
    /// view is gone they are ended (only the ones that use Pteron's own compatibility folder).
    fn clean_up_engine(&mut self) {
        let open = self.alive() > 0;
        if open {
            self.had_view = true;
            self.closed_at = None;
        } else if self.had_view {
            self.had_view = false;
            self.closed_at = Some(Instant::now());
            #[cfg(pteron_wry)]
            {
                self.context = None;
            }
        }
        if self.closed_at.is_some_and(|at| at.elapsed() >= Duration::from_secs(3)) {
            self.closed_at = None;
            end_engine_processes();
        }
    }

    /// Number of compatibility views that exist right now.
    pub(crate) fn alive(&self) -> usize {
        self.entries.values().filter(|e| e.is_open()).count()
    }

    /// The page a compatibility tab is on now (it may have followed links).
    pub(crate) fn current_url(&self, id: WebViewId) -> Option<String> {
        self.entries.get(&id).map(|e| e.url.lock_ok().clone()).filter(|u| !u.is_empty())
    }

    /// Bring the views in line with the tabs. `live`: every tab. `shown`: the active tab when it is
    /// a compatibility tab, with its page and the area (in points) it covers; `None` when nothing
    /// should be drawn over the page (another kind of tab, or a menu or the Settings page is open).
    pub(crate) fn sync(
        &mut self,
        window: &winit::window::Window,
        live: &[WebViewId],
        active: Option<(WebViewId, &url::Url)>,
        area: Option<egui::Rect>,
        points_to_pixels: f32,
        typing: bool,
    ) {
        self.entries.retain(|id, _| live.contains(id));
        self.released.retain(|id, at| live.contains(id) && at.elapsed() < Duration::from_secs(5));
        if self.trim_until.is_some_and(|until| Instant::now() >= until) {
            self.trim_until = None;
        }
        let shown = match (active, area) {
            (Some((id, placeholder)), Some(area)) => crate::compat_view::target(placeholder)
                .map(|page| (id, page, area))
                // (Servo is still loading the page it was handed: the placeholder is not shown again.)
                .filter(|(id, ..)| !self.released.contains_key(id)),
            _ => None,
        };
        let shown_id = shown.as_ref().map(|(id, ..)| *id);
        if shown.is_none() {
            // Nothing over the page: the last report from a view no longer means anything.
            POINTER.store(0, Relaxed);
        }
        // The native view keeps the keyboard after a click in the page, even when the next click is
        // in a Pteron text box: when one starts wanting the keyboard, take it back.
        // (This counts while the compatibility tab is the active one, also when the view is hidden
        // for the address bar, which is exactly when the keyboard has to come back.)
        let on_compat = active.is_some_and(|(_, placeholder)| crate::compat_view::target(placeholder).is_some());
        if on_compat && typing && !self.typing {
            window.focus_window();
            #[cfg(pteron_wry)]
            for entry in self.entries.values() {
                if let Some(view) = &entry.view {
                    let _ = view.focus_parent();
                }
            }
        }
        self.typing = typing;
        // The native view keeps the keyboard (and so the typing) after it is hidden or the tab is
        // switched: when it goes away, the keyboard goes back to Pteron's own window.
        if self.was_shown && shown_id.is_none() {
            window.focus_window();
            #[cfg(pteron_wry)]
            for entry in self.entries.values() {
                if let Some(view) = &entry.view {
                    let _ = view.focus_parent();
                }
            }
        }
        self.was_shown = shown_id.is_some();
        if let Some((id, page, area)) = shown {
            if self.entries.get(&id).is_some_and(|e| e.target != page.as_str()) {
                self.entries.remove(&id);
            }
            let entry = self.entries.entry(id).or_insert_with(|| Entry {
                #[cfg(pteron_wry)]
                view: None,
                url: Arc::new(Mutex::new(page.to_string())),
                hidden_since: None,
                target: page.to_string(),
                flags: Default::default(),
                shown: false,
                bounds: None,
                low: false,
                agent_asked: None,
            });
            entry.hidden_since = None;
            self.show(window, id, area, points_to_pixels);
        }
        let keep = Duration::from_secs(crate::memsaver::tab_sleep_secs(KEEP_HIDDEN_SECS));
        for (_, entry) in self.entries.iter_mut().filter(|(id, _)| Some(**id) != shown_id) {
            entry.hide(keep);
        }
        self.clean_up_engine();
    }

    #[cfg(pteron_wry)]
    fn show(&mut self, window: &winit::window::Window, id: WebViewId, area: egui::Rect, scale: f32) {
        let rect = (
            (area.min.x * scale) as i32,
            (area.min.y * scale) as i32,
            (area.width() * scale).max(1.0) as u32,
            (area.height() * scale).max(1.0) as u32,
        );
        let bounds = || wry::Rect {
            position: wry::dpi::PhysicalPosition::new(rect.0, rect.1).into(),
            size: wry::dpi::PhysicalSize::new(rect.2, rect.3).into(),
        };
        if self.entries.get(&id).is_some_and(|e| e.view.is_none()) {
            if CLEANING.load(Relaxed) {
                return;
            }
            let (url, flags) = (self.entries[&id].url.clone(), self.entries[&id].flags.clone());
            let Some(view) = self.create(window, &url, flags, bounds()) else {
                crate::compat_view::mark_broken();
                return;
            };
            if let Some(entry) = self.entries.get_mut(&id) {
                (entry.view, entry.shown, entry.bounds, entry.low) = (Some(view), true, Some(rect), false);
                entry.agent_asked = None;
            }
            crate::crashlog::note(&format!("compatibility tab opened ({} open)", self.alive()));
        }
        let Some(entry) = self.entries.get_mut(&id) else { return };
        let Some(view) = &entry.view else { return };
        if entry.bounds != Some(rect) && view.set_bounds(bounds()).is_ok() {
            entry.bounds = Some(rect);
        }
        if !entry.shown && view.set_visible(true).is_ok() {
            entry.shown = true;
        }
        // After a browser check passes, the check's scripts and caches are no longer needed: a
        // short "low memory" period makes the engine give them back.
        if entry.flags.trim_now.swap(false, Relaxed) {
            self.trim_until = Some(Instant::now() + Duration::from_secs(3));
        }
        let low = self.trim_until.is_some_and(|until| Instant::now() < until);
        if !low {
            self.trim_until = None;
        }
        if entry.low != low {
            #[cfg(windows)]
            {
                use wry::WebViewExtWindows;
                let _ = view.set_memory_usage_level(if low { wry::MemoryUsageLevel::Low } else { wry::MemoryUsageLevel::Normal });
            }
            entry.low = low;
        }
        // The engine's browser name, asked once the page runs (a script sent while the engine is
        // still starting is held back and its answer is lost, so this is tried again until it comes).
        if ENGINE_AGENT.lock_ok().is_empty() && entry.agent_asked.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
            entry.agent_asked = Some(Instant::now());
            let _ = view.evaluate_script_with_callback("navigator.userAgent", |json| {
                if let Ok(agent) = serde_json::from_str::<String>(&json) {
                    *ENGINE_AGENT.lock_ok() = agent;
                }
            });
        }
        // A real page loaded in this view: if the site passed a browser check, give the pass to
        // Servo (once the engine's browser name is known; until then this is tried again).
        let ready = entry.flags.clone();
        if ready.page_ready.load(Relaxed) &&
            self.handoff.is_none() &&
            crate::pteron::settings().compat_tab_handoff &&
            !ENGINE_AGENT.lock_ok().is_empty()
        {
            ready.page_ready.store(false, Relaxed);
            self.handoff = self.prepare_handoff(id);
        }
    }

    /// The cookies of the page the view is on, when they include a Cloudflare pass, and the engine's
    /// browser name; `None` when there is nothing to hand over (or the site already failed once).
    #[cfg(pteron_wry)]
    fn prepare_handoff(&self, id: WebViewId) -> Option<Handoff> {
        let entry = self.entries.get(&id)?;
        let view = entry.view.as_ref()?;
        let page = url::Url::parse(&view.url().ok()?).ok()?;
        let host = page.host_str()?;
        if crate::compat_view::stays(host) ||
            crate::compat_view::site_listed(&crate::pteron::settings().compat_tab_keep_sites, host)
        {
            return None;
        }
        let agent = ENGINE_AGENT.lock_ok().clone();
        let cookies = view.cookies_for_url(page.as_str()).ok()?;
        if !cookies.iter().any(|c| c.name() == "cf_clearance") {
            return None;
        }
        crate::crashlog::note(&format!("handing {} over to the light engine", page.host_str().unwrap_or("a site")));
        Some(Handoff { id, page, cookies: cookies.iter().map(|c| c.to_string()).collect(), agent })
    }

    #[cfg(not(pteron_wry))]
    fn show(&mut self, _: &winit::window::Window, _: WebViewId, _: egui::Rect, _: f32) {}

    #[cfg(pteron_wry)]
    fn create(&mut self, window: &winit::window::Window, url: &Arc<Mutex<String>>, flags: Arc<Flags>, bounds: wry::Rect) -> Option<wry::WebView> {
        #[cfg(target_os = "linux")]
        if !gtk_ready() {
            crate::crashlog::note("compatibility tab could not start: GTK did not start");
            return None;
        }
        let dir = dirs::data_dir()?.join(crate::pteron::APP_NAME).join("compat");
        let context = self.context.get_or_insert_with(|| wry::WebContext::new(Some(dir)));
        let follow = url.clone();
        let start = url.lock_ok().clone();
        let builder = wry::WebViewBuilder::new_with_web_context(context)
            .with_url(&start)
            .with_bounds(bounds)
            .with_initialization_script(POINTER_SCRIPT)
            .with_document_title_changed_handler(move |title| title_changed(&flags, &title))
            .with_ipc_handler(|request| {
                let state = match request.body().as_str() {
                    "in" => 1,
                    "edge" => 2,
                    _ => 0,
                };
                // (The page reports only changes.) Draw a frame so the sidebar opens or closes now.
                POINTER.store(state, Relaxed);
                crate::window::repaint_after(Duration::ZERO);
            })
            .with_navigation_handler(move |next| {
                if next.starts_with("http://") || next.starts_with("https://") {
                    *follow.lock_ok() = next;
                }
                true
            });
        #[cfg(windows)]
        let builder = {
            use wry::WebViewBuilderExtWindows;
            builder.with_additional_browser_args(browser_args())
        };
        match builder.build_as_child(window) {
            Ok(view) => Some(view),
            Err(error) => {
                crate::crashlog::note(&format!("compatibility tab could not start: {error}"));
                None
            },
        }
    }
}

impl Entry {
    fn is_open(&self) -> bool {
        #[cfg(pteron_wry)]
        return self.view.is_some();
        #[cfg(not(pteron_wry))]
        false
    }

    /// Not on screen: hide, ask for less memory, and drop the view after a while.
    fn hide(&mut self, keep: Duration) {
        #[cfg(pteron_wry)]
        if let Some(view) = &self.view {
            // Just went off screen (this runs every frame: the engine is only told once).
            if self.shown {
                let _ = view.set_visible(false);
                #[cfg(windows)]
                {
                    use wry::WebViewExtWindows;
                    let _ = view.set_memory_usage_level(wry::MemoryUsageLevel::Low);
                }
                (self.shown, self.low) = (false, true);
            }
            let since = *self.hidden_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= keep {
                if let Ok(now) = view.url() &&
                    now.starts_with("http")
                {
                    *self.url.lock_ok() = now;
                }
                (self.view, self.bounds, self.low) = (None, None, false);
                crate::crashlog::note("a hidden compatibility tab was closed to save memory (it reloads when opened)");
            }
        }
        #[cfg(not(pteron_wry))]
        let _ = (&self.hidden_since, keep);
    }
}

/// End the system engine's helper processes that were started for Pteron's compatibility folder.
#[cfg(windows)]
fn end_engine_processes() {
    let script = format!(
        "Get-CimInstance Win32_Process -Filter \"Name='msedgewebview2.exe'\" | \
         Where-Object {{ $_.CommandLine -like '*{}\\compat*' }} | \
         ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}",
        crate::pteron::APP_NAME
    );
    CLEANING.store(true, Relaxed);
    let started = std::thread::Builder::new().name("pteron-engine-cleanup".into()).spawn(move || {
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x0800_0000)
            .status();
        CLEANING.store(false, Relaxed);
        crate::window::repaint_after(Duration::ZERO);
    });
    if started.is_err() {
        CLEANING.store(false, Relaxed);
    }
}

#[cfg(not(windows))]
fn end_engine_processes() {}

