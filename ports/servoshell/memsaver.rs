//! Pteron: "Memory saver". Gives up some speed and comfort to use less memory, for older or
//! low-end computers (Settings > Tabs & memory).
//!
//! * the engine's thread pools and web cache are made smaller (applied at start-up),
//! * the video helper reads less of the video ahead of the player,
//! * unused tabs go to sleep sooner,
//! * memory the window no longer needs is handed back to Windows when Pteron is in the
//!   background.
//!
//! A reading of the memory in use is also written to the crash log now and then, so a
//! problem report shows how much Pteron was using over time.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Level {
    Normal,
    Low,
    Lowest,
}

impl Level {
    pub(crate) fn from_id(id: &str) -> Level {
        match id {
            "low" => Level::Low,
            "lowest" => Level::Lowest,
            _ => Level::Normal,
        }
    }

    pub(crate) fn id(self) -> &'static str {
        match self {
            Level::Normal => "normal",
            Level::Low => "low",
            Level::Lowest => "lowest",
        }
    }

    fn number(self) -> u8 {
        match self {
            Level::Normal => 0,
            Level::Low => 1,
            Level::Lowest => 2,
        }
    }

    fn from_number(n: u8) -> Level {
        match n {
            1 => Level::Low,
            2 => Level::Lowest,
            _ => Level::Normal,
        }
    }
}

/// What a level changes. `None` leaves the engine's own value alone.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Profile {
    pub layout_threads: Option<i64>,
    pub workers: Option<u64>,
    pub fallback_workers: Option<u64>,
    pub async_workers: Option<u64>,
    pub canvas_workers: Option<u64>,
    pub webrender_workers: Option<u64>,
    /// Size of the engine's in-memory web cache.
    pub http_cache: Option<u64>,
    /// How many video pieces, and at most how many bytes, the helper reads ahead.
    pub read_ahead_pieces: usize,
    pub read_ahead_bytes: u64,
    /// Unused tabs sleep after at most this many seconds.
    pub tab_sleep_cap_secs: Option<u64>,
    /// Seconds in the background before memory is handed back to Windows.
    pub trim_after_secs: Option<u64>,
    /// How many earlier pages per tab stay loaded for Back (older ones reload when visited).
    /// The engine keeps 20, each a whole page in memory. Never 0: the engine then keeps more.
    pub history_pages: i64,
    /// Frames per second for animations and scrolling (the engine's own default was 120).
    pub frame_rate: i64,
    /// At most this many background tabs stay awake; the longest unused sleep first.
    pub awake_tabs: Option<usize>,
}

const MB: u64 = 1024 * 1024;

pub(crate) fn profile(level: Level) -> Profile {
    match level {
        Level::Normal => Profile {
            layout_threads: None,
            workers: None,
            fallback_workers: None,
            async_workers: None,
            canvas_workers: None,
            webrender_workers: None,
            // The engine keeps up to 5000 responses in memory whatever their size; 2000 still
            // covers the last few dozen pages.
            http_cache: Some(2000),
            read_ahead_pieces: 5,
            read_ahead_bytes: 32 * MB,
            tab_sleep_cap_secs: None,
            trim_after_secs: None,
            history_pages: 2,
            frame_rate: 60,
            awake_tabs: None,
        },
        Level::Low => Profile {
            layout_threads: Some(2),
            workers: Some(3),
            fallback_workers: Some(2),
            async_workers: Some(4),
            canvas_workers: Some(2),
            webrender_workers: None,
            http_cache: Some(1500),
            read_ahead_pieces: 3,
            read_ahead_bytes: 16 * MB,
            tab_sleep_cap_secs: Some(180),
            trim_after_secs: Some(15),
            history_pages: 1,
            frame_rate: 60,
            awake_tabs: Some(8),
        },
        Level::Lowest => Profile {
            layout_threads: Some(1),
            workers: Some(2),
            fallback_workers: Some(2),
            async_workers: Some(2),
            canvas_workers: Some(1),
            webrender_workers: Some(2),
            http_cache: Some(300),
            read_ahead_pieces: 2,
            read_ahead_bytes: 8 * MB,
            tab_sleep_cap_secs: Some(60),
            trim_after_secs: Some(8),
            history_pages: 1,
            frame_rate: 30,
            awake_tabs: Some(4),
        },
    }
}

static LEVEL: AtomicU8 = AtomicU8::new(0);

pub(crate) fn set_level(level: Level) {
    LEVEL.store(level.number(), Ordering::Relaxed);
}

pub(crate) fn level() -> Level {
    Level::from_number(LEVEL.load(Ordering::Relaxed))
}

/// (pieces, bytes) the video helper may read ahead right now.
pub(crate) fn read_ahead() -> (usize, u64) {
    let p = profile(level());
    (p.read_ahead_pieces, p.read_ahead_bytes)
}

/// The time after which an unused tab sleeps, given the user's own setting in seconds
/// (0 = never). A memory saver level shortens it, and turns sleeping on if it was off.
pub(crate) fn tab_sleep_secs(user_secs: u64) -> u64 {
    match profile(level()).tab_sleep_cap_secs {
        Some(cap) if user_secs == 0 => cap,
        Some(cap) => user_secs.min(cap),
        None => user_secs,
    }
}

// ---------------------------------------------------------------- memory of the computer

/// Free (available) and total memory of the computer, in MB.
#[cfg(windows)]
pub(crate) fn system_memory_mb() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: the struct is plain numbers and the call only fills it in.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0)
        .then(|| (status.ullAvailPhys / MB, status.ullTotalPhys / MB))
}

#[cfg(target_os = "linux")]
pub(crate) fn system_memory_mb() -> Option<(u64, u64)> {
    parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn system_memory_mb() -> Option<(u64, u64)> {
    None
}

#[cfg(any(target_os = "linux", test))]
fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let kb = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.split_whitespace().next()?.parse::<u64>().ok())
    };
    Some((kb("MemAvailable:")? / 1024, kb("MemTotal:")? / 1024))
}

/// The computer is short of memory: less than a tenth of it is free (at least 400 MB and at
/// most 1 GB counts as "short", whatever the size of the computer).
pub(crate) fn memory_is_low(available_mb: u64, total_mb: u64) -> bool {
    available_mb < (total_mb / 10).clamp(400, 1024)
}

/// How many background tabs should go to sleep before their normal time: those over the level's
/// limit of awake tabs, and one more every ten seconds while the computer is short of memory.
pub(crate) fn tabs_to_sleep_early(awake_background_tabs: usize) -> usize {
    use crate::locks::LockExt;
    static LAST_CHECK: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let over_limit = profile(level())
        .awake_tabs
        .map_or(0, |limit| awake_background_tabs.saturating_sub(limit));
    let mut last = LAST_CHECK.lock_ok();
    if last.is_some_and(|at| at.elapsed() < Duration::from_secs(10)) {
        return over_limit;
    }
    *last = Some(std::time::Instant::now());
    drop(last);
    let low = system_memory_mb().is_some_and(|(available, total)| memory_is_low(available, total));
    if low && awake_background_tabs > 0 {
        crate::crashlog::note("memory: the computer is short of memory, a background tab sleeps early");
    }
    over_limit.max(usize::from(low))
}

// ---------------------------------------------------------------- background trimming

static FOCUS_ROUND: AtomicU64 = AtomicU64::new(0);

/// The window got or lost the keyboard focus. After a while in the background, memory the
/// window no longer needs is handed back to Windows (only when a saver level is on).
pub(crate) fn note_focus(focused: bool) {
    let round = FOCUS_ROUND.fetch_add(1, Ordering::SeqCst) + 1;
    if focused {
        return;
    }
    let Some(secs) = profile(level()).trim_after_secs else { return };
    let _ = std::thread::Builder::new().name("pteron-trim".into()).spawn(move || {
        std::thread::sleep(Duration::from_secs(secs));
        if FOCUS_ROUND.load(Ordering::SeqCst) == round {
            crate::window::trim_process_memory();
        }
    });
}

// ---------------------------------------------------------------- memory readings

static TABS: AtomicUsize = AtomicUsize::new(0);
static LOGGER_STARTED: AtomicBool = AtomicBool::new(false);

/// How many tabs are open (for the log).
pub(crate) fn set_tabs(count: usize) {
    TABS.store(count, Ordering::Relaxed);
}

/// Memory in use by Pteron in MB (the working set, close to what Task Manager shows).
#[cfg(windows)]
pub(crate) fn process_mb() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: the structure is zeroed and sized as the call requires, and the handle is the
    // current process, which the OS gives us.
    unsafe {
        let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) != 0 {
            Some(counters.WorkingSetSize as u64 / MB)
        } else {
            None
        }
    }
}

/// Linux: the resident memory from `/proc/<process>/statm` (second number, in pages), in bytes.
#[cfg(target_os = "linux")]
fn resident_bytes(process: &str) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{process}/statm")).ok()?;
    let pages: u64 = text.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: `sysconf` only reads a system constant.
    let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok().filter(|p| *p > 0).unwrap_or(4096);
    Some(pages * page)
}

#[cfg(target_os = "linux")]
pub(crate) fn process_mb() -> Option<u64> {
    resident_bytes("self").map(|bytes| bytes / MB)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn process_mb() -> Option<u64> {
    None
}

/// Memory in MB of the programs Pteron started: the system web engine behind compatibility tabs
/// (WebView2, WebKitGTK) runs as separate processes that `process_mb` does not see. Looked up
/// at most every two seconds.
pub(crate) fn helpers_mb() -> u64 {
    use crate::locks::LockExt;
    static LAST: std::sync::Mutex<Option<(std::time::Instant, u64)>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock_ok();
    if let Some((at, mb)) = *last &&
        at.elapsed() < Duration::from_secs(2)
    {
        return mb;
    }
    let mb = helper_bytes() / MB;
    *last = Some((std::time::Instant::now(), mb));
    mb
}

/// The processes started by `root`, directly or through others, from (process, parent) pairs.
fn descendants(root: u32, processes: &[(u32, u32)]) -> Vec<u32> {
    let mut found = vec![root];
    let mut next = 0;
    while next < found.len() {
        let children: Vec<u32> = processes
            .iter()
            .filter(|(process, parent)| *parent == found[next] && !found.contains(process))
            .map(|(process, _)| *process)
            .collect();
        found.extend(children);
        next += 1;
    }
    found.remove(0);
    found
}

#[cfg(windows)]
fn helper_bytes() -> u64 {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let mut processes = Vec::new();
    let mut bytes = 0;
    // SAFETY: the structures are zeroed and sized as the calls require, and every handle is
    // checked before use and closed afterwards.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return 0;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            processes.push((entry.th32ProcessID, entry.th32ParentProcessID));
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        for process in descendants(std::process::id(), &processes) {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process);
            if handle.is_null() {
                continue;
            }
            let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            counters.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            if K32GetProcessMemoryInfo(handle, &mut counters, counters.cb) != 0 {
                bytes += counters.WorkingSetSize as u64;
            }
            CloseHandle(handle);
        }
    }
    bytes
}

#[cfg(target_os = "linux")]
fn helper_bytes() -> u64 {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    let processes: Vec<(u32, u32)> = entries
        .flatten()
        .filter_map(|entry| {
            let process: u32 = entry.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            // "12 (name) S 7 ...": the name may hold spaces, so read after the last ')'.
            let parent = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()?;
            Some((process, parent))
        })
        .collect();
    descendants(std::process::id(), &processes)
        .iter()
        .filter_map(|process| resident_bytes(&process.to_string()))
        .sum()
}

#[cfg(not(any(windows, target_os = "linux")))]
fn helper_bytes() -> u64 {
    0
}

/// Write a line to the crash log when the memory changed by a fifth or ten minutes passed.
pub(crate) fn should_log(last_logged_mb: Option<u64>, now_mb: u64, secs_since_log: u64) -> bool {
    match last_logged_mb {
        None => true,
        Some(last) => {
            let diff = now_mb.abs_diff(last);
            diff * 5 >= last.max(1) || secs_since_log >= 600
        },
    }
}

/// Starts the once-per-half-minute memory reading (once).
pub(crate) fn start_logger() {
    if LOGGER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new().name("pteron-memory-log".into()).spawn(|| {
        let mut last: Option<(u64, std::time::Instant)> = None;
        loop {
            std::thread::sleep(Duration::from_secs(30));
            let Some(mb) = process_mb() else { return };
            let since = last.map(|(_, at)| at.elapsed().as_secs()).unwrap_or(0);
            if should_log(last.map(|(m, _)| m), mb, since) {
                let compat = match crate::compat_host::alive_count() {
                    0 => String::new(),
                    n => format!(", {n} compatibility tabs (system web engine: {} MB more)", helpers_mb()),
                };
                crate::crashlog::note(&format!(
                    "memory: {mb} MB in use, {} tabs, memory saver {}{compat}",
                    TABS.load(Ordering::Relaxed),
                    level().id()
                ));
                last = Some((mb, std::time::Instant::now()));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_unknown_means_normal() {
        for level in [Level::Normal, Level::Low, Level::Lowest] {
            assert_eq!(Level::from_id(level.id()), level);
            assert_eq!(Level::from_number(level.number()), level);
        }
        assert_eq!(Level::from_id("???"), Level::Normal);
    }

    #[test]
    fn normal_keeps_full_speed() {
        let p = profile(Level::Normal);
        assert!(p.layout_threads.is_none() && p.workers.is_none());
        assert!(p.http_cache.is_some_and(|n| n >= profile(Level::Low).http_cache.unwrap_or(0)));
        assert!(p.tab_sleep_cap_secs.is_none() && p.trim_after_secs.is_none());
        assert_eq!((p.read_ahead_pieces, p.read_ahead_bytes), (5, 32 * MB));
    }

    #[test]
    fn lower_levels_use_less() {
        let (low, lowest) = (profile(Level::Low), profile(Level::Lowest));
        assert!(low.layout_threads > lowest.layout_threads);
        assert!(low.http_cache > lowest.http_cache);
        assert!(low.read_ahead_bytes > lowest.read_ahead_bytes);
        assert!(low.tab_sleep_cap_secs > lowest.tab_sleep_cap_secs);
        // Nothing may be set to zero: a pool with no workers would hang the engine.
        for p in [low, lowest] {
            assert!(p.layout_threads.is_none_or(|n| n >= 1));
            for n in [p.workers, p.fallback_workers, p.async_workers, p.canvas_workers, p.webrender_workers] {
                assert!(n.is_none_or(|n| n >= 1));
            }
            assert!(p.read_ahead_pieces >= 1);
        }
        assert!(profile(Level::Normal).history_pages >= profile(Level::Low).history_pages);
        assert!([Level::Normal, Level::Low, Level::Lowest].iter().all(|l| profile(*l).history_pages >= 1));
        assert!([Level::Normal, Level::Low, Level::Lowest].iter().all(|l| (24..=60).contains(&profile(*l).frame_rate)));
        assert!(profile(Level::Normal).awake_tabs.is_none());
        assert!(profile(Level::Low).awake_tabs > profile(Level::Lowest).awake_tabs);
        assert!(profile(Level::Lowest).awake_tabs.is_some_and(|n| n >= 1));
    }

    #[test]
    fn finds_the_programs_pteron_started() {
        // 1 started 2 and 3; 3 started 4; 9 belongs to something else; 5 claims itself as parent.
        let processes = [(2, 1), (3, 1), (4, 3), (9, 8), (1, 0), (5, 5)];
        assert_eq!(descendants(1, &processes), vec![2, 3, 4]);
        assert!(descendants(9, &processes).is_empty());
        assert!(descendants(5, &processes).is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn counts_the_memory_of_a_started_program() {
        let mut child = std::process::Command::new("sleep").arg("5").spawn().unwrap();
        let bytes = helper_bytes();
        let _ = child.kill();
        let _ = child.wait();
        assert!(bytes > 0);
    }

    #[test]
    fn low_memory_is_recognised() {
        let text = "MemTotal:        8000000 kB\nMemFree:          100000 kB\nMemAvailable:    2048000 kB\n";
        assert_eq!(parse_meminfo(text), Some((2000, 7812)));
        assert_eq!(parse_meminfo("MemTotal: 5 kB\n"), None);
        // 8 GB: short below 800 MB. 2 GB: below 400 MB. 64 GB: below 1 GB.
        assert!(memory_is_low(700, 8000) && !memory_is_low(900, 8000));
        assert!(memory_is_low(350, 2000) && !memory_is_low(450, 2000));
        assert!(memory_is_low(1000, 64000) && !memory_is_low(1100, 64000));
    }

    #[test]
    fn tab_sleep_follows_the_level() {
        set_level(Level::Normal);
        assert_eq!((tab_sleep_secs(0), tab_sleep_secs(900)), (0, 900));
        set_level(Level::Lowest);
        assert_eq!((tab_sleep_secs(0), tab_sleep_secs(900), tab_sleep_secs(30)), (60, 60, 30));
        assert_eq!(read_ahead(), (2, 8 * MB));
        set_level(Level::Normal);
    }

    #[test]
    fn logging_rule() {
        assert!(should_log(None, 100, 0));
        assert!(!should_log(Some(100), 105, 30));
        assert!(should_log(Some(100), 130, 30));
        assert!(should_log(Some(100), 70, 30));
        assert!(should_log(Some(100), 101, 600));
    }
}
