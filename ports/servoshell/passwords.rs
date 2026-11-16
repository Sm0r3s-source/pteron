/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pteron: saved logins.
//!
//! A small script runs in every top-level page. When a login form is sent it reports the user
//! name and password through the page console; this module catches that message (it never
//! reaches the log), and the browser asks "Save password?". When a page with a password field
//! opens and logins are saved for the site, the browser offers to fill them. Nothing is ever
//! filled without a click.
//!
//! Passwords are encrypted before they touch the disk. On Windows with Windows' own protection
//! (DPAPI, tied to this Windows account), in `%APPDATA%\Pteron\passwords.json`. On Linux with
//! AES-256-GCM, under a random key kept in the system keyring (GNOME Keyring / KWallet, through
//! `secret-tool`), or, where there is no keyring, in a key file only your account can read
//! (`~/.config/Pteron/passwords.key`). Passwords are never synced and never put in problem
//! reports.

use crate::locks::LockExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use url::Url;

const SAVE_PREFIX: &str = "__pteron_pw__";
const FORM_MARK: &str = "__pteron_pwform__";

/// A secret for this run, put into the page script's messages. Frames inside a page (an ad, say)
/// can write to the same console but never get to run the script's reporting part, so they
/// cannot fake a "save this login" or "fill a login here" message.
fn token() -> &'static str {
    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TOKEN.get_or_init(|| {
        use ring::rand::SecureRandom;
        let mut bytes = [0u8; 12];
        let _ = ring::rand::SystemRandom::new().fill(&mut bytes);
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    })
}

fn save_prefix() -> String {
    format!("{SAVE_PREFIX}{}", token())
}

fn form_mark() -> String {
    format!("{FORM_MARK}{}", token())
}

/// The page script, with this run's secret in its messages.
pub(crate) fn script() -> String {
    SCRIPT.replace("P='__pteron_pw__'", &format!("P='{}'", save_prefix())).replace("F='__pteron_pwform__'", &format!("F='{}'", form_mark()))
}
const SAVE_WAIT: Duration = Duration::from_secs(120);
const MAX_FIELD: usize = 256;
const MAX_LOGINS: usize = 2000;

/// Runs at the start of every page. Only the top-level page is looked at, so a login form inside
/// an embedded frame can never be saved under the wrong site.
pub(crate) const SCRIPT: &str = r#"(function(){try{
if(window!==window.top||window.__pteronPw)return;window.__pteronPw=1;
var P='__pteron_pw__',F='__pteron_pwform__';
function vis(e){var r=e.getBoundingClientRect();return r.width>0&&r.height>0;}
function pws(root){return [].filter.call((root||document).querySelectorAll('input[type=password]'),vis);}
function userFor(pw){var root=pw.form||document,all=[].slice.call(root.querySelectorAll('input')),i=all.indexOf(pw);
for(var k=i-1;k>=0;k--){var e=all[k],t=(e.type||'text').toLowerCase();
if((t==='text'||t==='email'||t==='tel')&&e.value&&vis(e))return e.value;}return '';}
var last=0;
function report(root){try{var f=pws(root).filter(function(e){return e.value;});if(!f.length)return;
var pw=f[f.length-1],now=Date.now();if(now-last<1500)return;last=now;
console.log(P+JSON.stringify({u:userFor(pw),p:pw.value}));}catch(e){}}
document.addEventListener('submit',function(e){report(e.target);},true);
document.addEventListener('click',function(e){var b=e.target&&e.target.closest&&e.target.closest('button,input[type=submit],input[type=button],[role=button]');
if(b)report(b.form||b.closest('form')||document);},true);
document.addEventListener('keydown',function(e){if(e.key==='Enter'&&e.target&&e.target.type==='password')report(e.target.form||document);},true);
var told=false,n=0;
var timer=setInterval(function(){n++;if(!told&&pws().length){told=true;console.log(F);}
if(told||n>10)clearInterval(timer);},800);
}catch(e){}})();"#;

#[derive(Clone, Debug, PartialEq)]
struct Login {
    host: String,
    user: String,
    /// The password, protected by Windows and written in base64.
    pw: String,
}

#[derive(Debug, PartialEq)]
struct Vault {
    enabled: bool,
    /// Ask Windows (Hello: PIN, fingerprint or face) before a saved password is filled.
    ask_windows: bool,
    never: Vec<String>,
    logins: Vec<Login>,
}

impl Vault {
    fn to_json(&self) -> String {
        let logins: Vec<Value> = self
            .logins
            .iter()
            .map(|l| json!({"host": l.host, "user": l.user, "pw": l.pw}))
            .collect();
        serde_json::to_string_pretty(&json!({"enabled": self.enabled, "ask_windows": self.ask_windows, "never": self.never, "logins": logins}))
            .unwrap_or_default()
    }

    fn from_json(text: &str) -> Option<Vault> {
        let v: Value = serde_json::from_str(text).ok()?;
        let text_of = |x: &Value, key: &str| x.get(key).and_then(|s| s.as_str()).map(str::to_string);
        Some(Vault {
            enabled: v.get("enabled").and_then(|b| b.as_bool()).unwrap_or(true),
            ask_windows: v.get("ask_windows").and_then(|b| b.as_bool()).unwrap_or(false),
            never: v
                .get("never")
                .and_then(|n| n.as_array())
                .map(|n| n.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            logins: v
                .get("logins")
                .and_then(|l| l.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|x| {
                            Some(Login { host: text_of(x, "host")?, user: text_of(x, "user")?, pw: text_of(x, "pw")? })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

impl Default for Vault {
    fn default() -> Self {
        Vault { enabled: true, ask_windows: false, never: Vec::new(), logins: Vec::new() }
    }
}

struct Pending {
    host: String,
    user: String,
    password: String,
    at: Instant,
}

struct Offer {
    host: String,
    page: String,
    at: Instant,
}

struct Ready {
    host: String,
    script: String,
}

#[derive(Default)]
struct State {
    vault: Option<Vault>,
    pending: Option<Pending>,
    offer: Option<Offer>,
    ready: Option<Ready>,
    busy: bool,
    message: Option<(String, Instant)>,
}

static STATE: Mutex<State> = Mutex::new(State {
    vault: None,
    pending: None,
    offer: None,
    ready: None,
    busy: false,
    message: None,
});

// ---------------------------------------------------------------- sites

/// The key a login is filed under: the host without `www.` (and the port when there is one).
/// Only secure pages (and the user's own computer) are used.
pub(crate) fn site_key(url: &Url) -> Option<String> {
    let host = url.host_str()?.to_ascii_lowercase();
    let local = host == "localhost" || host == "127.0.0.1";
    if !(url.scheme() == "https" || (url.scheme() == "http" && local)) {
        return None;
    }
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

// ---------------------------------------------------------------- the file

#[cfg(not(any(target_os = "android", target_env = "ohos")))]
fn file_path() -> Option<PathBuf> {
    ::dirs::config_dir().map(|dir| dir.join(crate::pteron::APP_NAME).join("passwords.json"))
}

#[cfg(any(target_os = "android", target_env = "ohos"))]
fn file_path() -> Option<PathBuf> {
    None
}

fn load() -> Vault {
    file_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| Vault::from_json(&text))
        .unwrap_or_default()
}

fn store(vault: &Vault) {
    let Some(path) = file_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = vault.to_json();
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

fn with_vault<R>(f: impl FnOnce(&mut Vault) -> R) -> R {
    let mut state = STATE.lock_ok();
    let vault = state.vault.get_or_insert_with(load);
    f(vault)
}

fn save_vault_now() {
    let state = STATE.lock_ok();
    if let Some(vault) = &state.vault {
        store(vault);
    }
}

// ---------------------------------------------------------------- Windows protection

#[cfg(windows)]
fn dpapi(protect: bool, input_b64: &str) -> Result<String, String> {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    let action = if protect {
        "[Convert]::ToBase64String([Security.Cryptography.ProtectedData]::Protect($b,$e,'CurrentUser'))"
    } else {
        "[Convert]::ToBase64String([Security.Cryptography.ProtectedData]::Unprotect($b,$e,'CurrentUser'))"
    };
    let script = format!(
        "$ErrorActionPreference='Stop';Add-Type -AssemblyName System.Security;\
         $e=[Text.Encoding]::UTF8.GetBytes('Pteron-passwords-v1');\
         $b=[Convert]::FromBase64String([Console]::In.ReadToEnd().Trim());{action}"
    );
    let mut child = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script.as_str()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|e| format!("Could not start Windows protection: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input_b64.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !text.is_empty() {
        Ok(text)
    } else {
        Err("Windows could not protect or open the password.".into())
    }
}

#[cfg(windows)]
fn protect(plain: &str) -> Result<String, String> {
    dpapi(true, &crate::util::b64_encode(plain.as_bytes()))
}

#[cfg(windows)]
fn unprotect(blob: &str) -> Result<String, String> {
    let bytes = crate::util::b64_decode(&dpapi(false, blob)?).ok_or("Unreadable password.")?;
    String::from_utf8(bytes).map_err(|_| "Unreadable password.".to_string())
}

// ---------------------------------------------------------------- Linux protection

#[cfg(not(windows))]
fn protect(plain: &str) -> Result<String, String> {
    // A new key is made only while no password is locked with one, so a keyring that is
    // locked or away for a moment never leads to a second key (and lost passwords).
    let create = !with_vault(|v| v.logins.iter().any(|l| l.pw.starts_with(SEALED)));
    seal(&keystore::key(create)?, plain.as_bytes())
}

#[cfg(not(windows))]
fn unprotect(blob: &str) -> Result<String, String> {
    let bytes = open_sealed(&keystore::key(false)?, blob)?;
    String::from_utf8(bytes).map_err(|_| "Unreadable password.".to_string())
}

/// How a password locked with AES-256-GCM starts in the file.
const SEALED: &str = "g1:";
const SEAL_LABEL: &[u8] = b"Pteron-passwords-v1";

/// Encrypt with AES-256-GCM: `g1:` and base64 of a random 12-byte nonce, the text and the tag.
#[cfg_attr(windows, allow(dead_code))]
fn seal(key: &[u8; 32], plain: &[u8]) -> Result<String, String> {
    use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
    use ring::rand::SecureRandom;
    let sealer = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(|_| "Bad key.".to_string())?);
    let mut nonce = [0u8; 12];
    ring::rand::SystemRandom::new().fill(&mut nonce).map_err(|_| "No random numbers.".to_string())?;
    let mut data = plain.to_vec();
    sealer
        .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(SEAL_LABEL), &mut data)
        .map_err(|_| "Could not lock the password.".to_string())?;
    let mut out = nonce.to_vec();
    out.extend(data);
    Ok(format!("{SEALED}{}", crate::util::b64_encode(&out)))
}

/// The other way round; fails if the key is wrong or the text was changed.
#[cfg_attr(windows, allow(dead_code))]
fn open_sealed(key: &[u8; 32], blob: &str) -> Result<Vec<u8>, String> {
    use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
    let unreadable = || "This password was saved on another computer or with another key, and cannot be opened.".to_string();
    let data = blob.strip_prefix(SEALED).and_then(crate::util::b64_decode).ok_or_else(unreadable)?;
    if data.len() < 12 + 16 {
        return Err(unreadable());
    }
    let (nonce, rest) = data.split_at(12);
    let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| unreadable())?;
    let opener = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(|_| unreadable())?);
    let mut rest = rest.to_vec();
    let plain = opener.open_in_place(nonce, Aad::from(SEAL_LABEL), &mut rest).map_err(|_| unreadable())?;
    Ok(plain.to_vec())
}

/// The key that locks saved passwords on Linux: in the system keyring through `secret-tool`
/// (from libsecret), or in a file only this account can read when there is no keyring.
#[cfg(not(windows))]
mod keystore {
    use std::io::{Read, Write};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use crate::locks::LockExt;

    static KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);
    const ATTRIBUTES: [&str; 4] = ["application", "pteron", "purpose", "saved-passwords"];

    fn decode(text: &str) -> Option<[u8; 32]> {
        crate::util::b64_decode(text.trim())?.try_into().ok()
    }

    /// Run `secret-tool` with `input` on its standard input; its output, if it worked. Gives up
    /// after a minute (the keyring may ask you to unlock it first).
    fn secret_tool(args: &[&str], input: Option<&str>) -> Option<String> {
        let mut child = Command::new("secret-tool")
            .args(args)
            .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            let _ = stdin.write_all(input.as_bytes());
        }
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut out = String::new();
                    child.stdout.take()?.read_to_string(&mut out).ok()?;
                    return status.success().then_some(out);
                },
                Ok(None) if started.elapsed() < Duration::from_secs(60) => std::thread::sleep(Duration::from_millis(50)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                },
            }
        }
    }

    fn from_keyring() -> Option<[u8; 32]> {
        let mut args = vec!["lookup"];
        args.extend(ATTRIBUTES);
        decode(&secret_tool(&args, None)?)
    }

    fn into_keyring(key: &[u8; 32]) -> bool {
        let mut args = vec!["store", "--label=Pteron saved passwords"];
        args.extend(ATTRIBUTES);
        secret_tool(&args, Some(&crate::util::b64_encode(key))).is_some() && from_keyring() == Some(*key)
    }

    fn key_file() -> Option<PathBuf> {
        ::dirs::config_dir().map(|dir| dir.join(crate::pteron::APP_NAME).join("passwords.key"))
    }

    fn from_file() -> Option<[u8; 32]> {
        decode(&std::fs::read_to_string(key_file()?).ok()?)
    }

    /// A new key file that only this account can read or write.
    fn into_file(key: &[u8; 32]) -> bool {
        let Some(path) = key_file() else { return false };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options
            .open(&path)
            .and_then(|mut file| file.write_all(crate::util::b64_encode(key).as_bytes()))
            .is_ok()
    }

    /// The key (asked for once per run). With `create`, a new key is made when there is none.
    pub(super) fn key(create: bool) -> Result<[u8; 32], String> {
        let mut cached = KEY.lock_ok();
        if let Some(key) = *cached {
            return Ok(key);
        }
        let found = from_keyring().or_else(from_file);
        let key = match found {
            Some(key) => key,
            None if create => {
                use ring::rand::SecureRandom;
                let mut key = [0u8; 32];
                ring::rand::SystemRandom::new().fill(&mut key).map_err(|_| "No random numbers.".to_string())?;
                if !into_keyring(&key) && !into_file(&key) {
                    return Err("Pteron could not keep a key for your saved passwords (no keyring, and the settings folder is not writable).".into());
                }
                key
            },
            None => {
                return Err("The key for your saved passwords was not found. If your system keyring is locked, unlock it and try again.".into());
            },
        };
        *cached = Some(key);
        Ok(key)
    }

    /// Where the key is kept, for the settings page.
    pub(super) fn place() -> &'static str {
        if key_file().is_some_and(|path| path.exists()) {
            "a key file only your account can read"
        } else {
            "your system keyring"
        }
    }
}

// ---------------------------------------------------------------- page messages

/// Called for every console message. Returns true when it was one of ours (the message is then
/// not logged, since it can hold a password). `page` is the address of the tab it came from.
pub(crate) fn handle_console(page: Option<&Url>, message: &str) -> bool {
    if let Some(json) = message.strip_prefix(&save_prefix()) {
        if let Some(page) = page {
            on_login_sent(page, json);
        }
        return true;
    }
    if message.trim() == form_mark() {
        if let Some(page) = page {
            on_form_seen(page);
        }
        return true;
    }
    false
}

fn on_login_sent(page: &Url, json: &str) {
    // Passwords are only protected on Windows for now: elsewhere nothing is captured.
    if !cfg!(windows) {
        return;
    }
    let Some(host) = site_key(page) else { return };
    let Ok(sent) = serde_json::from_str::<Value>(json) else { return };
    let field = |key: &str| sent.get(key).and_then(|v| v.as_str()).map(str::to_string);
    let (Some(user), Some(password)) = (field("u"), field("p")) else { return };
    let sent = (user, password);
    if sent.1.is_empty() || sent.1.chars().count() > MAX_FIELD || sent.0.chars().count() > MAX_FIELD {
        return;
    }
    let mut state = STATE.lock_ok();
    let vault = state.vault.get_or_insert_with(load);
    if !vault.enabled || vault.never.contains(&host) {
        return;
    }
    state.pending = Some(Pending { host, user: sent.0, password: sent.1, at: Instant::now() });
}

fn on_form_seen(page: &Url) {
    // Passwords are only protected on Windows for now: elsewhere nothing is captured.
    if !cfg!(windows) {
        return;
    }
    let Some(host) = site_key(page) else { return };
    let mut state = STATE.lock_ok();
    let vault = state.vault.get_or_insert_with(load);
    if !vault.enabled || !vault.logins.iter().any(|l| l.host == host) {
        return;
    }
    state.offer = Some(Offer { host, page: page.as_str().to_string(), at: Instant::now() });
}

// ---------------------------------------------------------------- asking the user

/// What the "Save password?" bar should say: (site, user name, whether it replaces an old one).
pub(crate) fn save_prompt() -> Option<(String, String, bool)> {
    let mut state = STATE.lock_ok();
    if state.pending.as_ref().is_some_and(|p| p.at.elapsed() > SAVE_WAIT) {
        state.pending = None;
    }
    let pending = state.pending.as_ref()?;
    let (host, user) = (pending.host.clone(), pending.user.clone());
    let vault = state.vault.get_or_insert_with(load);
    let exists = vault.logins.iter().any(|l| l.host == host && l.user == user);
    Some((host, user, exists))
}

pub(crate) fn dismiss_save(never_for_site: bool) {
    let mut state = STATE.lock_ok();
    if let Some(pending) = state.pending.take() &&
        never_for_site
    {
        let vault = state.vault.get_or_insert_with(load);
        if !vault.never.contains(&pending.host) {
            vault.never.push(pending.host);
        }
        drop(state);
        save_vault_now();
    }
}

/// Encrypt and file the password the bar asked about. Runs in the background.
pub(crate) fn accept_save() {
    let Some(pending) = STATE.lock_ok().pending.take() else { return };
    std::thread::Builder::new()
        .name("pteron-passwords".into())
        .spawn(move || {
            let result = protect(&pending.password);
            let mut state = STATE.lock_ok();
            match result {
                Ok(blob) => {
                    let vault = state.vault.get_or_insert_with(load);
                    vault.logins.retain(|l| !(l.host == pending.host && l.user == pending.user));
                    if vault.logins.len() < MAX_LOGINS {
                        vault.logins.push(Login { host: pending.host.clone(), user: pending.user, pw: blob });
                    }
                    drop(state);
                    save_vault_now();
                    say(format!("Password saved for {}.", pending.host));
                },
                Err(message) => {
                    drop(state);
                    say(message);
                },
            }
        })
        .ok();
}

fn say(text: String) {
    STATE.lock_ok().message = Some((text, Instant::now()));
}

/// A short result message ("Password saved...") for a few seconds.
pub(crate) fn message() -> Option<String> {
    let mut state = STATE.lock_ok();
    match &state.message {
        Some((text, at)) if at.elapsed() < Duration::from_secs(4) => Some(text.clone()),
        Some(_) => {
            state.message = None;
            None
        },
        None => None,
    }
}

/// The user names that can be filled on `page`, when the page just showed a password field.
pub(crate) fn fill_offer(page: &Url) -> Option<(String, Vec<String>)> {
    let mut state = STATE.lock_ok();
    let offer = state.offer.as_ref()?;
    if offer.at.elapsed() > Duration::from_secs(600) || offer.page != page.as_str() {
        return None;
    }
    let host = offer.host.clone();
    let vault = state.vault.get_or_insert_with(load);
    let users: Vec<String> = vault.logins.iter().filter(|l| l.host == host).map(|l| l.user.clone()).collect();
    (!users.is_empty()).then_some((host, users))
}

pub(crate) fn dismiss_offer() {
    STATE.lock_ok().offer = None;
}

/// The script that puts a login into the page.
fn fill_script(user: &str, password: &str, site: &str) -> String {
    let user = serde_json::to_string(user).unwrap_or_else(|_| "\"\"".into());
    let password = serde_json::to_string(password).unwrap_or_else(|_| "\"\"".into());
    let site = serde_json::to_string(site).unwrap_or_else(|_| "\"\"".into());
    // The page itself checks that it is still the site the login belongs to (it may have moved
    // on in the moment between the click and the fill).
    format!(
        "(function(u,p,s){{try{{var h=location.hostname.toLowerCase().replace(/^www\\./,'')+(location.port?':'+location.port:'');\
         var local=/^(localhost|127\\.0\\.0\\.1)(:|$)/.test(h);\
         if(window!==window.top||h!==s||!(location.protocol==='https:'||(local&&location.protocol==='http:')))return;\
         function vis(e){{var r=e.getBoundingClientRect();return r.width>0&&r.height>0;}}\
         function set(el,v){{var d=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value');\
         if(d&&d.set)d.set.call(el,v);else el.value=v;\
         el.dispatchEvent(new Event('input',{{bubbles:true}}));el.dispatchEvent(new Event('change',{{bubbles:true}}));}}\
         var pw=[].filter.call(document.querySelectorAll('input[type=password]'),vis)[0];if(!pw)return;\
         var root=pw.form||document,all=[].slice.call(root.querySelectorAll('input')),i=all.indexOf(pw);\
         if(u){{for(var k=i-1;k>=0;k--){{var e=all[k],t=(e.type||'text').toLowerCase();\
         if((t==='text'||t==='email'||t==='tel')&&vis(e)){{set(e,u);break;}}}}}}\
         set(pw,p);}}catch(e){{}}}})({user},{password},{site});"
    )
}

/// Start opening login number `index` (of those for the offered site) in the background.
pub(crate) fn request_fill(index: usize) {
    let (host, blob, user, ask) = {
        let mut state = STATE.lock_ok();
        let Some(host) = state.offer.take().map(|o| o.host) else { return };
        if state.busy {
            return;
        }
        let vault = state.vault.get_or_insert_with(load);
        let Some(login) = vault.logins.iter().filter(|l| l.host == host).nth(index).cloned() else { return };
        let ask = vault.ask_windows;
        state.busy = true;
        (host, login.pw, login.user, ask)
    };
    std::thread::Builder::new()
        .name("pteron-passwords".into())
        .spawn(move || {
            let result = confirm_user(ask, &host).and_then(|()| unprotect(&blob));
            let mut state = STATE.lock_ok();
            state.busy = false;
            match result {
                Ok(password) => {
                    let script = fill_script(&user, &password, &host);
                    state.ready = Some(Ready { host, script })
                },
                Err(message) => state.message = Some((message, Instant::now())),
            }
        })
        .ok();
}

/// A fill script that is ready for the page at `page`, if any (and only for the same site).
pub(crate) fn take_fill_script(page: &Url) -> Option<String> {
    let mut state = STATE.lock_ok();
    let ready = state.ready.take()?;
    (site_key(page).as_deref() == Some(ready.host.as_str())).then_some(ready.script)
}

/// True while something is working in the background (so the window keeps repainting).
pub(crate) fn working() -> bool {
    let state = STATE.lock_ok();
    state.busy || state.ready.is_some()
}

// ---------------------------------------------------------------- settings page

/// When switched on, Windows asks for the user's PIN, fingerprint or face (Windows Hello) before a
/// saved password is filled; a confirmation counts for five minutes.
fn confirm_user(ask: bool, host: &str) -> Result<(), String> {
    static CONFIRMED: Mutex<Option<Instant>> = Mutex::new(None);
    if !ask || CONFIRMED.lock_ok().is_some_and(|at| at.elapsed() < Duration::from_secs(300)) {
        return Ok(());
    }
    windows_hello(&format!("Pteron: fill your saved password for {host}"))?;
    *CONFIRMED.lock_ok() = Some(Instant::now());
    Ok(())
}

#[cfg(windows)]
fn windows_hello(message: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    // The message goes in through standard input, so nothing in it is read as PowerShell.
    const SCRIPT: &str = "$ErrorActionPreference='Stop';\
        Add-Type -AssemblyName System.Runtime.WindowsRuntime;\
        $null=[Windows.Security.Credentials.UI.UserConsentVerifier,Windows.Security.Credentials.UI,ContentType=WindowsRuntime];\
        $asTask=[System.WindowsRuntimeSystemExtensions].GetMethods()|Where-Object{$_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'}|Select-Object -First 1;\
        function Wait-Op($op,$type){$t=$asTask.MakeGenericMethod($type).Invoke($null,@($op));$null=$t.Wait(-1);$t.Result};\
        $msg=[Console]::In.ReadToEnd().Trim();\
        $a=Wait-Op ([Windows.Security.Credentials.UI.UserConsentVerifier]::CheckAvailabilityAsync()) ([Windows.Security.Credentials.UI.UserConsentVerifierAvailability]);\
        if(([string]$a) -ne 'Available'){'unavailable:'+$a;exit 0};\
        $r=Wait-Op ([Windows.Security.Credentials.UI.UserConsentVerifier]::RequestVerificationAsync($msg)) ([Windows.Security.Credentials.UI.UserConsentVerificationResult]);\
        'result:'+$r";
    let mut child = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", SCRIPT])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|e| format!("Could not ask Windows to confirm it is you: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(message.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let answer = String::from_utf8_lossy(&out.stdout).trim().to_string();
    match answer.as_str() {
        "result:Verified" => Ok(()),
        a if a.starts_with("unavailable:") => Err(
            "Windows Hello (a PIN, fingerprint or face) is not set up, so Pteron cannot ask it is you. \
             Set it up in Windows Settings, or switch the question off in Pteron's password settings."
                .into(),
        ),
        "result:Canceled" => Err("Not filled: Windows did not confirm it is you.".into()),
        _ => Err("Not filled: Windows could not confirm it is you.".into()),
    }
}

#[cfg(not(windows))]
fn windows_hello(_message: &str) -> Result<(), String> {
    Err("Saved passwords only work on Windows for now.".into())
}

pub(crate) fn asks_windows() -> bool {
    with_vault(|v| v.ask_windows)
}

pub(crate) fn set_asks_windows(on: bool) {
    with_vault(|v| v.ask_windows = on);
    save_vault_now();
}

/// Saved passwords work on Windows and Linux.
pub(crate) fn enabled() -> bool {
    supported() && with_vault(|v| v.enabled)
}

pub(crate) fn supported() -> bool {
    cfg!(any(windows, target_os = "linux"))
}

/// Where the key that locks saved passwords is kept, said for the settings page.
pub(crate) fn protection() -> &'static str {
    #[cfg(windows)]
    return "your Windows account";
    #[cfg(not(windows))]
    return keystore::place();
}

pub(crate) fn set_enabled(on: bool) {
    with_vault(|v| v.enabled = on);
    save_vault_now();
}

/// Every saved login as (site, user name).
pub(crate) fn list() -> Vec<(String, String)> {
    with_vault(|v| v.logins.iter().map(|l| (l.host.clone(), l.user.clone())).collect())
}

pub(crate) fn delete(index: usize) {
    with_vault(|v| {
        if index < v.logins.len() {
            v.logins.remove(index);
        }
    });
    save_vault_now();
}

pub(crate) fn delete_all() {
    with_vault(|v| v.logins.clear());
    save_vault_now();
}

pub(crate) fn never_list() -> Vec<String> {
    with_vault(|v| v.never.clone())
}

pub(crate) fn allow_again(index: usize) {
    with_vault(|v| {
        if index < v.never.len() {
            v.never.remove(index);
        }
    });
    save_vault_now();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn sealed_passwords_open_only_with_their_key() {
        let key = [7u8; 32];
        let blob = seal(&key, "hunter2 \u{e9}".as_bytes()).unwrap();
        assert!(blob.starts_with(SEALED) && !blob.contains("hunter2"));
        assert_eq!(open_sealed(&key, &blob).unwrap(), "hunter2 \u{e9}".as_bytes());
        assert_ne!(seal(&key, b"same").unwrap(), seal(&key, b"same").unwrap(), "a new nonce every time");
        assert!(open_sealed(&[8u8; 32], &blob).is_err(), "wrong key");
        let mut bytes = crate::util::b64_decode(&blob[3..]).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(open_sealed(&key, &format!("{SEALED}{}", crate::util::b64_encode(&bytes))).is_err(), "changed text");
        assert!(open_sealed(&key, "g1:AAAA").is_err());
    }

    #[test]
    fn site_keys() {
        assert_eq!(site_key(&url("https://www.Example.com/login")).as_deref(), Some("example.com"));
        assert_eq!(site_key(&url("https://example.com:8443/")).as_deref(), Some("example.com:8443"));
        assert_eq!(site_key(&url("http://example.com/")), None);
        assert_eq!(site_key(&url("http://localhost:3000/")).as_deref(), Some("localhost:3000"));
        assert_eq!(site_key(&url("file:///a.html")), None);
    }

    #[test]
    fn fill_script_is_quoted() {
        let script = fill_script("a\"b</script>", "p'w\\\n", "a.com");
        assert!(script.contains("\"a\\\"b</script>\""));
        assert!(script.contains("\"p'w\\\\\\n\""));
        assert!(!script.contains('\n'));
    }

    #[test]
    fn vault_file_round_trip() {
        let vault = Vault {
            enabled: false,
            ask_windows: true,
            never: vec!["bank.example".into()],
            logins: vec![Login { host: "a.com".into(), user: "me".into(), pw: "AAAA".into() }],
        };
        let text = vault.to_json();
        assert_eq!(Vault::from_json(&text).unwrap(), vault);
        let old = Vault::from_json("{}").unwrap();
        assert!(old.enabled && old.logins.is_empty());
    }

    #[test]
    fn page_messages_are_ours_and_checked() {
        // Not ours: left alone.
        assert!(!handle_console(Some(&url("https://a.com/")), "hello"));
        // Ours, but from a page we do not save for: swallowed, nothing stored.
        assert!(handle_console(Some(&url("http://a.com/")), &format!("{}{{\"u\":\"x\",\"p\":\"y\"}}", save_prefix())));
        assert!(handle_console(None, &form_mark()));
        // Without this run's secret (a frame inside the page, say): not ours, nothing happens.
        assert!(!handle_console(Some(&url("https://a.com/")), "__pteron_pw__{\"u\":\"x\",\"p\":\"y\"}"));
        assert!(!handle_console(Some(&url("https://a.com/")), "__pteron_pwform__"));
        // A page's own script cannot make the bar for a different site: the site comes from the
        // tab's address, not from the message.
        let long = format!("{}{{\"u\":\"x\",\"p\":\"{}\"}}", save_prefix(), "z".repeat(400));
        assert!(handle_console(Some(&url("https://a.com/")), &long));
        assert!(!STATE.lock_ok().pending.as_ref().is_some_and(|p| p.password.len() > MAX_FIELD));
    }

    #[test]
    fn script_only_runs_on_top_pages() {
        assert!(SCRIPT.contains("window!==window.top"));
        let script = script();
        assert!(script.contains(&format!("P='{}'", save_prefix())) && script.contains(&format!("F='{}'", form_mark())));
        assert!(!script.contains("P='__pteron_pw__'"));
    }
}
