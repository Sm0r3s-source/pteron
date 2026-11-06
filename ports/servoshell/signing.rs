//! Update signatures: an update is only installed if its SHA-256 is signed with the author's key.
//! The public half of the key is built into Pteron; the private half never leaves the author's PC.

use ring::signature::{ED25519, UnparsedPublicKey};

/// The author's public key (base64), written by `pteron_sign.ps1 -Init` before the build.
const PUBLIC_KEY: &str = include_str!("update_key.pub");

/// True when `signature` (base64) is a valid signature of `message` under `public_key` (base64).
pub(crate) fn verify_with(public_key: &str, message: &[u8], signature: &str) -> bool {
    let (Some(key), Some(sig)) = (
        crate::util::b64_decode(public_key.trim()),
        crate::util::b64_decode(signature.trim()),
    ) else {
        return false;
    };
    UnparsedPublicKey::new(&ED25519, key).verify(message, &sig).is_ok()
}

/// True when `hash` (the zip's SHA-256 as hex) is signed by the built-in key.
pub(crate) fn verify_hash(hash: &str, signature: &str) -> bool {
    verify_with(PUBLIC_KEY, hash.as_bytes(), signature)
}

#[cfg(test)]
mod tests {
    use ring::signature::{Ed25519KeyPair, KeyPair};

    use super::*;

    #[test]
    fn accepts_a_signature_made_by_the_signing_tool() {
        // Made by pteron_sign.ps1's tool for the text "abcd".
        let public = "f2U+W3bg8p0V1ITCnxqwJrNHxRVIadXBX9hF/KTUDoU=";
        let sig = "xzkjG9EZ0fF+BRJd3FBzxI8TvAQUhKdqjocavOpkg8MHoCTz1KI4s75oqJN9wOkY00tjmgu73cAi1HkSCErnDg==";
        assert!(verify_with(public, b"abcd", sig));
        assert!(!verify_with(public, b"abce", sig));
    }

    #[test]
    fn accepts_only_the_matching_message_and_key() {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public = crate::util::b64_encode(pair.public_key().as_ref());
        let hash = "ab".repeat(32);
        let sig = crate::util::b64_encode(pair.sign(hash.as_bytes()).as_ref());
        assert!(verify_with(&public, hash.as_bytes(), &sig));
        assert!(!verify_with(&public, "cd".repeat(32).as_bytes(), &sig));
        assert!(!verify_with(&public, hash.as_bytes(), "not a signature"));
        let other = Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap().as_ref()).unwrap();
        assert!(!verify_with(&crate::util::b64_encode(other.public_key().as_ref()), hash.as_bytes(), &sig));
        assert!(!verify_with("", hash.as_bytes(), &sig));
    }
}
