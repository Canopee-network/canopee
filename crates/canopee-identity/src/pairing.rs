//! Pairing primitives for the LAN device-pairing protocol (Phase 3).
//!
//! The flow: the *new* device generates a 12-character base32 pairing code and
//! publishes a `PairingQrData` (code hash + device info + one-time `session_id`)
//! out of band (QR on screen / printed to a terminal). The *existing* device's
//! user types the code to approve, and the existing device encrypts the
//! identity + shared records under a session key derived from the code and
//! session id. Only the device that saw the code can decrypt.
//!
//! Security notes:
//! - The 12-char code carries ~60 bits of entropy — strong enough that an
//!   online guess isn't practical, weak enough to type.
//! - The code itself is *never* transmitted over the peer-to-peer channel; the
//!   AEAD key is derived from it (plus the session id and both device ids), so
//!   an eavesdropper on the wire learns nothing reusable.
//! - Session keys are single-use: the new device re-derives a fresh code +
//!   `session_id` for each `initiate_pairing` call.
//!
//! The same KDF + AEAD construction as encrypted-at-rest identity files
//! (Argon2id → XChaCha20-Poly1305) is used so the guarantees are consistent
//! across the codebase.

use anyhow::Result;
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, OsRng};
use chacha20poly1305::{AeadCore, KeyInit, XChaCha20Poly1305, XNonce};
use rand_core::RngCore;

/// Key length of the derived session key / cipher in bytes (256 bits).
const KEY_LEN: usize = 32;
/// Randomized 192-bit nonce length in bytes; never reused across encrypt calls.
const NONCE_LEN: usize = 24;
/// Length of the one-time session id in bytes (128 bits).
const SESSION_ID_LEN: usize = 16;

/// RFC 4648 base32 alphabet — a pairing code is a 12-character base32 string.
const CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// Characters per code: 12 × 5 = 60 bits of entropy.
const CODE_LEN: usize = 12;

/// Generates a fresh 12-character base32 pairing code (60 bits) using the OS
/// CSPRNG. Uniform sampling: each character is a masked byte (`& 31`), which
/// is exact because 32 divides 256.
pub fn generate_code() -> String {
    let mut random = [0u8; CODE_LEN];
    OsRng.fill_bytes(&mut random);
    random
        .iter()
        .map(|b| CODE_ALPHABET[(b & 31) as usize] as char)
        .collect()
}

/// Generates a one-time, unguessable session id (128 bits, hex-encoded) that
/// scopes a pairing exchange and is mixed into the KDF salt so the derived key
/// is different every time even if the same code is reused.
pub fn generate_session_id() -> String {
    let mut random = [0u8; SESSION_ID_LEN];
    OsRng.fill_bytes(&mut random);
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(SESSION_ID_LEN * 2);
    for byte in &random {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Constant-time comparison of the two pairing codes. Timing-independent on
/// the compared data; both strings are fixed-length so length-leak is moot.
pub fn codes_match(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Derives the 256-bit XChaCha20 session key from the pairing code and the
/// per-session salt (domain label + session id + both device ids, built by the
/// caller). Argon2id makes offline brute-force of a leaked code expensive.
pub fn derive_session_key(code: &str, salt: &[u8]) -> [u8; KEY_LEN] {
    let params = Params::default();
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon2
        .hash_password_into(code.as_bytes(), salt, &mut key)
        .expect("argon2 params are valid");
    key
}

/// Encrypts `clear` under `session_key`, framing it as
/// `nonce (24 bytes) || ciphertext || tag`. The nonce is freshly randomized
/// per call so the same key material never produces the same ciphertext.
pub fn encrypt_payload(clear: &[u8], session_key: &[u8; KEY_LEN]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(session_key.into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, clear)
        .map_err(|e| anyhow::anyhow!("pairing payload encryption failed: {e}"))?;

    let mut framed = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    framed.extend_from_slice(&nonce);
    framed.extend_from_slice(&ciphertext);
    Ok(framed)
}

/// Decrypts a payload produced by [`encrypt_payload`]. Fails on any
/// tampering or on a wrong session key (i.e. wrong pairing code).
pub fn decrypt_payload(framed: &[u8], session_key: &[u8; KEY_LEN]) -> Result<Vec<u8>> {
    if framed.len() < NONCE_LEN {
        anyhow::bail!("pairing payload is truncated");
    }
    let (nonce, ciphertext) = framed.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(session_key.into());
    let nonce = XNonce::from_slice(nonce);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("wrong pairing code or tampered pairing payload"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn generated_codes_are_12_base32_chars() {
        let code = generate_code();
        assert_eq!(code.len(), 12);
        assert!(
            code.chars().all(|c| CODE_ALPHABET.contains(&(c as u8))),
            "every char must come from the base32 alphabet"
        );
    }

    #[test]
    fn generated_codes_are_unique_and_high_entropy() {
        let codes: HashSet<String> = (0..100).map(|_| generate_code()).collect();
        assert_eq!(codes.len(), 100, "100 draws must not collide at 60 bits");
    }

    #[test]
    fn session_ids_are_distinct() {
        let a = generate_session_id();
        let b = generate_session_id();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }

    #[test]
    fn codes_match_is_constant_time_and_case_sensitive() {
        let code = generate_code();
        assert!(codes_match(&code, &code), "identical codes must match");
        assert!(!codes_match(&code, "ZZZZZZZZZZZZ"), "different codes must not match");
        assert!(!codes_match(&code, &code.to_lowercase()), "base32 is case-sensitive");
        assert!(!codes_match(&code, &code[..11]), "length mismatch must not match");
    }

    #[test]
    fn payload_round_trips_only_with_the_right_key() {
        let code = generate_code();
        let salt = generate_session_id().into_bytes();
        let key = derive_session_key(&code, &salt);
        let clear = b"identity key || device list || profile || contacts".to_vec();

        let framed = encrypt_payload(&clear, &key).unwrap();

        let other_code = generate_code();
        let other_key = derive_session_key(&other_code, &salt);
        assert!(
            decrypt_payload(&framed, &other_key).is_err(),
            "a different best-guess code must not decrypt"
        );

        let round_trip = decrypt_payload(&framed, &key).unwrap();
        assert_eq!(round_trip, clear);
    }

    #[test]
    fn payload_is_tamper_evident() {
        let code = generate_code();
        let key = derive_session_key(&code, &generate_session_id().into_bytes());
        let framed = encrypt_payload(b"secret", &key).unwrap();

        let mut tampered = framed.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert!(decrypt_payload(&tampered, &key).is_err());

        assert!(decrypt_payload(b"too short", &key).is_err());
    }

    #[test]
    fn derived_keys_depend_on_session_id() {
        let code = generate_code();
        let salt_a = generate_session_id().into_bytes();
        let salt_b = generate_session_id().into_bytes();
        assert_ne!(
            derive_session_key(&code, &salt_a),
            derive_session_key(&code, &salt_b),
            "different session ids must produce different keys"
        );
    }
}