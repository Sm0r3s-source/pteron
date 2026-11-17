//! Sirocco: private tabs go through Tor, and Tor only runs while a private tab is open.
//!
//! The private network client (`net::connector::private_connect`) can only connect through a local
//! SOCKS5 gateway. The gateway is `sirocco-tor`, a small separate program (it carries the Tor
//! client) that sits next to Sirocco. The first private tab starts it, the last one closing stops it,
//! and until it says it is up, private tabs cannot connect at all (they never fall back to a direct
//! connection). It also exits by itself if Sirocco goes away.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

#[derive(Clone, PartialEq)]
pub(crate) enum Status {
    Off,
    Starting,
    Ready,
    Failed(String),
}

struct Tor {
    users: usize,
    child: Option<Child>,
}

static TOR: Mutex<Tor> = Mutex::new(Tor { users: 0, child: None });
static STATUS: Mutex<Status> = Mutex::new(Status::Off);

fn set_status(status: Status) {
    if let Ok(mut current) = STATUS.lock() {
        *current = status;
    }
}

pub(crate) fn status() -> Status {
    STATUS.lock().map(|status| status.clone()).unwrap_or(Status::Off)
}

/// A private tab was opened.
pub(crate) fn acquire() {
    let Ok(mut tor) = TOR.lock() else { return };
    tor.users += 1;
    if tor.users == 1 {
        tor.child = start();
    }
}

/// A private tab was closed.
pub(crate) fn release() {
    let Ok(mut tor) = TOR.lock() else { return };
    tor.users = tor.users.saturating_sub(1);
    if tor.users == 0 {
        net::connector::set_private_gateway(None);
        if let Some(mut child) = tor.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        set_status(Status::Off);
    }
}

/// `SIROCCO_PRIVATE_PROXY=127.0.0.1:9050` makes private tabs use that SOCKS5 proxy (your own Tor or
/// VPN gateway, or a test one) instead of the built-in Tor.
fn own_proxy() -> Option<std::net::SocketAddr> {
    std::env::var("SIROCCO_PRIVATE_PROXY").ok()?.parse().ok()
}

fn gateway_program() -> Option<std::path::PathBuf> {
    let name = if cfg!(windows) { "sirocco-tor.exe" } else { "sirocco-tor" };
    let path = std::env::current_exe().ok()?.parent()?.join(name);
    path.is_file().then_some(path)
}

fn start() -> Option<Child> {
    if let Some(address) = own_proxy() {
        net::connector::set_private_gateway(Some(address));
        set_status(Status::Ready);
        return None;
    }
    let Some(program) = gateway_program() else {
        set_status(Status::Failed("the Tor part (sirocco-tor) is missing".into()));
        return None;
    };
    let (Some(data), Some(cache)) = (dirs::data_local_dir(), dirs::cache_dir()) else {
        set_status(Status::Failed("no folder for Tor".into()));
        return None;
    };
    let state = data.join(crate::pteron::APP_NAME).join("tor");
    let cache = cache.join(crate::pteron::APP_NAME).join("tor");
    let mut command = Command::new(program);
    command.arg(state).arg(cache).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // no console window
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            set_status(Status::Failed(error.to_string()));
            return None;
        },
    };
    set_status(Status::Starting);
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(address) = line.strip_prefix("LISTEN ") {
                    if let Ok(address) = address.trim().parse() {
                        net::connector::set_private_gateway(Some(address));
                    }
                } else if line.trim() == "READY" {
                    set_status(Status::Ready);
                } else if let Some(why) = line.strip_prefix("FAILED ") {
                    set_status(Status::Failed(why.trim().to_string()));
                }
            }
            // The gateway program ended: nothing may connect any more.
            net::connector::set_private_gateway(None);
            if status() != Status::Off {
                set_status(Status::Failed("Tor stopped".into()));
            }
        });
    }
    Some(child)
}

/// A short line for the "Private" badge.
pub(crate) fn badge_suffix() -> &'static str {
    match status() {
        Status::Ready => " · Tor",
        Status::Starting => " · connecting to Tor…",
        Status::Failed(_) => " · Tor unavailable",
        Status::Off => "",
    }
}
