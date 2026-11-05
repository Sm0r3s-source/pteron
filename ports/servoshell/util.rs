/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pteron: small helpers shared by several modules (base64 and random codes).

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

/// 32 hex digits that cannot be guessed (random per process, mixed with the clock).
pub(crate) fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    (0..2)
        .map(|_| {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u128(nanos);
            format!("{:016x}", hasher.finish())
        })
        .collect()
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
