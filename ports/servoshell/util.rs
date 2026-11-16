/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pteron: small helpers shared by several modules (base64, random codes, private temp folder).

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub(crate) fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char } else { '=' });
        }
    }
    out
}

/// Base64 in the standard or the URL-safe alphabet; stops at the first `=`. `None` for other characters.
pub(crate) fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let (mut out, mut buffer, mut bits) = (Vec::new(), 0u32, 0);
    for c in text.trim().bytes() {
        let value = match c {
            b'=' => break,
            b'-' => 62,
            b'_' => 63,
            _ => ALPHABET.iter().position(|a| *a == c)? as u32,
        };
        buffer = buffer << 6 | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// 32 hex digits that cannot be guessed (from the system's secure random source).
pub(crate) fn random_token() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    if ring::rand::SystemRandom::new().fill(&mut bytes).is_err() {
        // Practically never; still unguessable from outside the process.
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        bytes[..8].copy_from_slice(&hasher.finish().to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A folder for Pteron's temporary files that other users of the computer cannot reach.
/// Windows' temp folder is already per user; on Linux the shared `/tmp` is not, so
/// `~/.cache/Pteron/tmp` (only the owner may enter) is used instead.
pub(crate) fn private_temp(name: &str) -> std::path::PathBuf {
    #[cfg(windows)]
    return std::env::temp_dir().join(name);
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let base = dirs::cache_dir()
            .unwrap_or_else(|| std::env::temp_dir().join(format!("pteron-{}", unsafe { libc::getuid() })))
            .join("Pteron")
            .join("tmp");
        let _ = std::fs::create_dir_all(&base);
        let _ = std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700));
        base.join(name)
    }
}

/// Linux: makes Pteron's folders (settings, history, cookies, saved pages) owner-only, so other
/// users of the same computer cannot read them even where home folders are open to everyone.
/// Windows' own folders are already per user.
pub(crate) fn lock_down_folders() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let app = "Pteron";
        let folders = [
            dirs::config_dir().map(|d| d.join(app)),
            dirs::data_dir().map(|d| d.join(app)),
            dirs::cache_dir().map(|d| d.join(app)),
            dirs::config_dir().map(|d| d.join("servo")),
            dirs::data_dir().map(|d| d.join("servo")),
        ];
        for folder in folders.into_iter().flatten() {
            let _ = std::fs::create_dir_all(&folder);
            let _ = std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700));
        }
    }
}

/// 16 hex digits identifying `text` (FNV-1a), for file and page names.
pub(crate) fn fnv_hex(text: &str) -> String {
    let hash = text
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3));
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_and_long() {
        let (a, b) = (random_token(), random_token());
        assert!(a.len() == 32 && a != b && a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[cfg(unix)]
    #[test]
    fn private_temp_is_only_for_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = private_temp("pteron_test_private");
        assert!(!dir.starts_with("/tmp"));
        let mode = std::fs::metadata(dir.parent().unwrap()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn base64_round_trips_and_matches_known_values() {
        for text in ["", "a", "ab", "abc", "abcd", "p\u{e4}ssw\u{f6}rd \u{1f512}"] {
            assert_eq!(b64_decode(&b64_encode(text.as_bytes())).unwrap(), text.as_bytes());
        }
        assert_eq!(b64_encode(b"Man"), "TWFu");
        assert_eq!(b64_encode(b"Ma"), "TWE=");
        assert_eq!(b64_encode(b"M"), "TQ==");
        assert_eq!(b64_decode("aHR0cHM6Ly9leGFtcGxlLm5ldC9wYWdl").unwrap(), b"https://example.net/page");
        assert_eq!(b64_decode("-_-_").unwrap(), b64_decode("+/+/").unwrap());
        assert!(b64_decode("a$b").is_none());
    }

    #[test]
    fn names_are_stable() {
        assert_eq!(fnv_hex("a"), fnv_hex("a"));
        assert_ne!(fnv_hex("a"), fnv_hex("b"));
        assert_eq!(fnv_hex("").len(), 16);
    }

    #[test]
    fn tokens_differ_and_are_hex() {
        let a = random_token();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, random_token());
    }
}
