/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Sirocco: bring saved logins over from another browser.
//!
//! Two ways: a CSV file (what Chrome, Edge, Brave, Firefox, Opera, Bitwarden and most password
//! managers export), and, on Windows, reading Chrome, Edge and Brave directly. Only passwords
//! come over; logged-in sessions (cookies) cannot be copied, current Chromium locks them to its
//! own app. Nothing leaves this computer.

/// One login to import: site address, user name, password.
pub(crate) type Entry = (String, String, String);

/// Split CSV text (quotes, doubled quotes, line breaks inside quotes) into rows.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let (mut rows, mut row, mut field, mut quoted) = (Vec::new(), Vec::new(), String::new(), false);
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            },
            ('"', _) => quoted = !quoted,
            (',', false) => row.push(std::mem::take(&mut field)),
            ('\r', false) => {},
            ('\n', false) => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            },
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// The logins in an exported CSV. The columns are found by name, so the Chrome, Edge, Firefox,
/// Opera and Bitwarden layouts all work. `None` when it does not look like a password export.
pub(crate) fn from_csv(text: &str) -> Option<Vec<Entry>> {
    let rows = parse_csv(text);
    let header: Vec<String> = rows.first()?.iter().map(|h| h.trim().to_ascii_lowercase()).collect();
    let find = |names: &[&str]| header.iter().position(|h| names.contains(&h.as_str()));
    let url = find(&["url", "login_uri", "origin", "website", "web site", "hostname"])?;
    let user = find(&["username", "login_username", "user", "login", "user name"])?;
    let pass = find(&["password", "login_password"])?;
    Some(
        rows.iter()
            .skip(1)
            .filter_map(|r| {
                let (u, n, p) = (r.get(url)?.trim(), r.get(user)?.trim(), r.get(pass)?);
                (!u.is_empty() && !p.is_empty()).then(|| (u.to_string(), n.to_string(), p.clone()))
            })
            .collect(),
    )
}

/// Open a Chromium password: `v10` or `v11`, a 12-byte nonce, the text and a 16-byte tag, AES-256-GCM.
#[cfg_attr(not(windows), allow(dead_code))]
fn chromium_open(key: &[u8], blob: &[u8]) -> Option<String> {
    use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
    if blob.len() < 3 + 12 + 16 || !(blob.starts_with(b"v10") || blob.starts_with(b"v11")) {
        return None;
    }
    let (nonce, rest) = blob[3..].split_at(12);
    let opener = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?);
    let mut data = rest.to_vec();
    let plain = opener.open_in_place(Nonce::try_assume_unique_for_key(nonce).ok()?, Aad::empty(), &mut data).ok()?;
    String::from_utf8(plain.to_vec()).ok()
}

/// What a direct import found.
#[cfg(windows)]
pub(crate) struct Found {
    pub entries: Vec<Entry>,
    /// Passwords that could not be opened (newer app-bound lock, or a different Windows account).
    pub skipped: usize,
}

/// Windows' own protection (DPAPI) on many lines of base64 with one PowerShell start. A line that
/// fails comes back empty.
#[cfg(windows)]
pub(crate) fn dpapi_lines(entropy: Option<&str>, protect: bool, lines: &[String]) -> Result<Vec<String>, String> {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    let (method, entropy) = (
        if protect { "Protect" } else { "Unprotect" },
        entropy.map_or("$null".to_string(), |e| format!("[Text.Encoding]::UTF8.GetBytes('{e}')")),
    );
    let script = format!(
        "Add-Type -AssemblyName System.Security;$e={entropy};\
         foreach($l in [Console]::In.ReadToEnd().Split([char]10)){{$l=$l.Trim();if($l -eq ''){{continue}};\
         try{{[Convert]::ToBase64String([Security.Cryptography.ProtectedData]::{method}([Convert]::FromBase64String($l),$e,'CurrentUser'))}}catch{{''}}}}"
    );
    let mut child = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script.as_str()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|e| format!("Could not start Windows protection: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(lines.join("\n").as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let result: Vec<String> = text.lines().map(|l| l.trim().to_string()).collect();
    if result.len() == lines.len() { Ok(result) } else { Err("Windows could not open the passwords.".into()) }
}

/// The browsers that can be read directly: name and the folder holding `Local State`.
#[cfg(windows)]
pub(crate) fn browsers() -> Vec<(&'static str, std::path::PathBuf)> {
    let Some(base) = dirs::data_local_dir() else { return Vec::new() };
    [
        ("Chrome", "Google/Chrome/User Data"),
        ("Edge", "Microsoft/Edge/User Data"),
        ("Brave", "BraveSoftware/Brave-Browser/User Data"),
    ]
    .into_iter()
    .map(|(name, path)| (name, base.join(path)))
    .filter(|(_, dir)| dir.join("Local State").is_file())
    .collect()
}

/// Read every profile's saved logins of a Chromium browser.
#[cfg(windows)]
pub(crate) fn from_chromium(dir: &std::path::Path) -> Result<Found, String> {
    let state = std::fs::read_to_string(dir.join("Local State")).map_err(|e| e.to_string())?;
    let state: serde_json::Value = serde_json::from_str(&state).map_err(|e| e.to_string())?;
    let sealed_key = state["os_crypt"]["encrypted_key"].as_str().ok_or("This browser has no saved key.")?;
    let sealed_key = crate::util::b64_decode(sealed_key).ok_or("Unreadable key.")?;
    let sealed_key = sealed_key.strip_prefix(b"DPAPI").ok_or("This browser locks its passwords in a way Sirocco cannot open.")?;
    let key = dpapi_lines(None, false, &[crate::util::b64_encode(sealed_key)])?
        .pop()
        .and_then(|k| crate::util::b64_decode(&k))
        .filter(|k| k.len() == 32)
        .ok_or("Windows would not open this browser's key (is it another Windows account?).")?;

    let mut found = Found { entries: Vec::new(), skipped: 0 };
    let mut any_file = false;
    for profile in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let name = profile.file_name().to_string_lossy().to_string();
        if name != "Default" && !name.starts_with("Profile ") {
            continue;
        }
        let db = profile.path().join("Login Data");
        if !db.is_file() {
            continue;
        }
        any_file = true;
        // The browser keeps the file open: work on a copy.
        let copy = std::env::temp_dir().join(format!("sirocco-import-{}.db", std::process::id()));
        std::fs::copy(&db, &copy).map_err(|_| "Could not read the logins. Close that browser completely and try again.".to_string())?;
        let rows = read_logins(&copy);
        let _ = std::fs::remove_file(&copy);
        for (url, user, blob) in rows.map_err(|e| e.to_string())? {
            match chromium_open(&key, &blob) {
                Some(pass) if !url.is_empty() && !pass.is_empty() => found.entries.push((url, user, pass)),
                _ => found.skipped += 1,
            }
        }
    }
    if !any_file {
        return Err("No saved logins found in that browser.".into());
    }
    Ok(found)
}

#[cfg(windows)]
fn read_logins(path: &std::path::Path) -> rusqlite::Result<Vec<(String, String, Vec<u8>)>> {
    let db = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut query = db.prepare("SELECT origin_url, username_value, password_value FROM logins")?;
    let rows = query.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let list = rows.collect();
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_csv() {
        let csv = "name,url,username,password,note\nexample.com,https://example.com/login,ann,\"p,\"\"w\"\"\nd\",\nbad,,x,y,\n";
        let list = from_csv(csv).unwrap();
        assert_eq!(list, vec![("https://example.com/login".into(), "ann".into(), "p,\"w\"\nd".into())]);
    }

    #[test]
    fn firefox_and_bitwarden_csv() {
        let firefox = "\"url\",\"username\",\"password\",\"httpRealm\"\n\"https://a.org\",\"bob\",\"pw\",\"\"\n";
        assert_eq!(from_csv(firefox).unwrap(), vec![("https://a.org".into(), "bob".into(), "pw".into())]);
        let bitwarden = "folder,favorite,type,name,login_uri,login_username,login_password\n,,login,x,https://b.org,cy,secret\r\n";
        assert_eq!(from_csv(bitwarden).unwrap(), vec![("https://b.org".into(), "cy".into(), "secret".into())]);
        assert!(from_csv("a,b\n1,2\n").is_none());
    }

    #[test]
    fn chromium_blob() {
        use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
        let key = [7u8; 32];
        let mut data = b"hunter2".to_vec();
        LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).unwrap())
            .seal_in_place_append_tag(Nonce::assume_unique_for_key([1u8; 12]), Aad::empty(), &mut data)
            .unwrap();
        let mut blob = b"v10".to_vec();
        blob.extend([1u8; 12]);
        blob.extend(data);
        assert_eq!(chromium_open(&key, &blob).as_deref(), Some("hunter2"));
        assert_eq!(chromium_open(&[8u8; 32], &blob), None);
        assert_eq!(chromium_open(&key, b"v20xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"), None);
    }
}
