//! Recipient-scoped encryption for stored objects.
//!
//! An object stored through [`crate::Identity::encrypt_for_recipients`] holds
//! only ciphertext: at rest on this machine, in the export bundle, on any
//! relay, and in every peer's cache it replicates into. Only holders of a
//! recipient's X25519 private key can read it.
//!
//! # Why this is per-identity and not per-device
//!
//! Pairing copies the *same* identity key onto every device, and the X25519
//! secret is derived deterministically from that key
//! ([`crate::Identity::dh_public_key`]). So all of one person's devices share
//! one DH public key, and encrypting to it means "every device this person
//! owns can read this, and nobody else can" — the multi-device and the
//! end-to-end property come from the same key, with no per-device key
//! distribution to manage.
//!
//! # Scheme
//!
//! One random 256-bit *content key* encrypts the payload once; the content
//! key is then wrapped separately for each recipient with an ephemeral-static
//! X25519 exchange, so adding or removing a recipient never re-encrypts the
//! payload and never reuses a content key across recipients.
//!
//! ```text
//! envelope := "CNP1" | version:u8 | flags:u8 | n_recipients:u16le
//!             salt[8] | content_nonce[24]
//!             ( recipient_ephemeral_pub[32] | wrap_nonce[24] | wrapped_content_key[48] )*
//!             ciphertext
//! ```
//!
//! Per recipient the wrapping key is
//! `HKDF-SHA256(ikm = X25519(ephemeral_secret, recipient_pub), salt = salt, info = "canopee/object-wrap/v1" || ephemeral_pub || recipient_pub)`.
//! The `info` binds the derived key to *both* public keys, so a wrapped
//! content key cannot be replayed under a different recipient, and the random
//! per-envelope `salt` keeps every envelope's wrapping keys distinct.

use crate::Identity;
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand_core::RngCore;
use sha2::Sha256;
use x25519_dalek::{PublicKey as DhPublicKey, StaticSecret as StaticDhSecret};
use zeroize::Zeroize;

const MAGIC: &[u8; 4] = b"CNP1";
const VERSION: u8 = 1;
const SALT_LEN: usize = 8;
const CONTENT_NONCE_LEN: usize = 24;
const WRAP_NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;
const TAG_LEN: usize = 16;
const WRAPPED_KEY_LEN: usize = KEY_LEN + TAG_LEN;
const EPK_LEN: usize = 32;
const PER_RECIPIENT_LEN: usize = EPK_LEN + WRAP_NONCE_LEN + WRAPPED_KEY_LEN;

/// Fixed-length prefix, used to tell an envelope from legacy plaintext.
const HEADER_LEN: usize = 4 + 1 + 1 + 2 + SALT_LEN + CONTENT_NONCE_LEN;

const WRAP_INFO: &[u8] = b"canopee/object-wrap/v1";

/// Whether `bytes` is an object envelope rather than legacy cleartext.
///
/// Legacy objects written before encryption existed stay readable: they simply
/// don't carry the magic prefix and are returned as-is by the decrypt path.
pub fn is_envelope(bytes: &[u8]) -> bool {
    bytes.len() > HEADER_LEN && &bytes[..4] == MAGIC
}

/// Everything that went wrong reading an envelope.
#[derive(Debug, thiserror::Error)]
pub enum EnvelopeError {
    #[error("this object is not encrypted")]
    NotEncrypted,
    #[error("unsupported envelope version {0}")]
    UnsupportedVersion(u8),
    #[error("the encrypted object is malformed: {0}")]
    Malformed(&'static str),
    #[error("no recipient key in this envelope matches this identity")]
    NoMatchingRecipient,
    #[error("decryption failed: the object is corrupt or was encrypted to someone else")]
    DecryptFailed,
}

impl Identity {
    /// Encrypts `plaintext` so that this identity — and therefore every device
    /// carrying it — plus each of `also_for` can read it, and nobody else.
    ///
    /// This identity's own DH key is always included. That is deliberate: an
    /// object nobody can read is unrecoverable data loss, and "I can always
    /// read what I stored" is the property a personal node needs.
    ///
    /// Returns the serialized envelope. See the [module docs](self) for the
    /// layout.
    pub fn encrypt_for_recipients(
        &self,
        plaintext: &[u8],
        also_for: &[[u8; 32]],
    ) -> Result<Vec<u8>, EnvelopeError> {
        let mut recipient_pubs: Vec<[u8; 32]> = vec![self.dh_public_key()];
        for pub_key in also_for {
            // The owner's own key may be passed explicitly once a device list
            // or contact list starts naming it; dedup rather than wrapping twice.
            if *pub_key != recipient_pubs[0] && !recipient_pubs.contains(pub_key) {
                recipient_pubs.push(*pub_key);
            }
        }

        let mut content_key = [0u8; KEY_LEN];
        OsRng.fill_bytes(&mut content_key);

        let mut salt = [0u8; SALT_LEN];
        OsRng.fill_bytes(&mut salt);
        let content_nonce = random_bytes(CONTENT_NONCE_LEN)?;

        let cipher = XChaCha20Poly1305::new((&content_key).into());
        let ciphertext = cipher
            .encrypt(XNonce::from_slice(&content_nonce), plaintext)
            .map_err(|_| EnvelopeError::Malformed("content encryption failed"))?;

        let mut out = Vec::with_capacity(
            HEADER_LEN + recipient_pubs.len() * PER_RECIPIENT_LEN + ciphertext.len(),
        );
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.push(0); // flags, reserved
        out.extend_from_slice(&(recipient_pubs.len() as u16).to_le_bytes());
        out.extend_from_slice(&salt);
        out.extend_from_slice(&content_nonce);

        for recipient_pub in &recipient_pubs {
            // Ephemeral-static X25519: the recipient derives the same shared
            // secret from its own private key and this public key, and the
            // ephemeral private key never leaves this function.
            //
            // The bytes come from `rand_core` 0.6's `OsRng` (the same CSPRNG
            // the rest of this crate uses); x25519-dalek's own
            // `random_from_rng` needs the 0.9 trait, so build the secret from
            // raw bytes instead. Clamping is applied by x25519-dalek.
            let mut esk_bytes = [0u8; 32];
            OsRng.fill_bytes(&mut esk_bytes);
            let ephemeral_secret = StaticDhSecret::from(esk_bytes);
            esk_bytes.zeroize();
            let epk = DhPublicKey::from(&ephemeral_secret).to_bytes();
            let shared = ephemeral_secret.diffie_hellman(&DhPublicKey::from(*recipient_pub));

            let wrap_key = derive_wrap_key(shared.as_bytes(), &salt, &epk, recipient_pub);
            let wrap_nonce = random_bytes(WRAP_NONCE_LEN)?;
            let wrap_cipher = XChaCha20Poly1305::new((&wrap_key).into());
            let wrapped = wrap_cipher
                .encrypt(XNonce::from_slice(&wrap_nonce), content_key.as_slice())
                .map_err(|_| EnvelopeError::Malformed("content key wrap failed"))?;

            out.extend_from_slice(&epk);
            out.extend_from_slice(&wrap_nonce);
            out.extend_from_slice(&wrapped);
        }

        out.extend_from_slice(&ciphertext);

        content_key.zeroize();
        Ok(out)
    }

    /// Decrypts an envelope produced by [`Self::encrypt_for_recipients`],
    /// returning the original plaintext.
    ///
    /// Returns [`EnvelopeError::NotEncrypted`] for legacy cleartext objects so
    /// callers can pass pre-encryption objects straight through.
    pub fn decrypt_envelope(&self, bytes: &[u8]) -> Result<Vec<u8>, EnvelopeError> {
        if !is_envelope(bytes) {
            return Err(EnvelopeError::NotEncrypted);
        }

        let version = bytes[4];
        if version != VERSION {
            return Err(EnvelopeError::UnsupportedVersion(version));
        }
        let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        if count == 0 {
            return Err(EnvelopeError::Malformed("envelope has no recipients"));
        }

        let salt = &bytes[8..8 + SALT_LEN];
        let content_nonce = &bytes[8 + SALT_LEN..HEADER_LEN];

        let recipients_end = HEADER_LEN
            .checked_add(
                count
                    .checked_mul(PER_RECIPIENT_LEN)
                    .ok_or(EnvelopeError::Malformed("recipient count overflows"))?,
            )
            .ok_or(EnvelopeError::Malformed("envelope length overflows"))?;
        if bytes.len() < recipients_end {
            return Err(EnvelopeError::Malformed("envelope is truncated"));
        }

        let ciphertext = &bytes[recipients_end..];

        let mut content_key: Option<[u8; KEY_LEN]> = None;
        for i in 0..count {
            let base = HEADER_LEN + i * PER_RECIPIENT_LEN;
            let epk = &bytes[base..base + EPK_LEN];
            let wrap_nonce = &bytes[base + EPK_LEN..base + EPK_LEN + WRAP_NONCE_LEN];
            let wrapped = &bytes[base + EPK_LEN + WRAP_NONCE_LEN..base + PER_RECIPIENT_LEN];

            let epk_array: [u8; EPK_LEN] = epk.try_into().expect("sliced to EPK_LEN");
            // A low-order / all-zero public key would yield an all-zero shared
            // secret; x25519-dalek's high-bit clamping means such a point never
            // produces a usable secret, so reject it outright.
            if *DhPublicKey::from(epk_array).as_bytes() == [0u8; 32] {
                continue;
            }
            let shared = self
                .dh_secret()
                .diffie_hellman(&DhPublicKey::from(epk_array));
            let wrap_key =
                derive_wrap_key(shared.as_bytes(), salt, &epk_array, &self.dh_public_key());

            let wrap_cipher = XChaCha20Poly1305::new((&wrap_key).into());
            if let Ok(key) = wrap_cipher.decrypt(XNonce::from_slice(wrap_nonce), wrapped)
                && key.len() == KEY_LEN
            {
                content_key = Some(key.as_slice().try_into().expect("checked length"));
                break;
            }
        }

        let Some(content_key) = content_key else {
            return Err(EnvelopeError::NoMatchingRecipient);
        };
        let cipher = XChaCha20Poly1305::new((&content_key).into());
        cipher
            .decrypt(XNonce::from_slice(content_nonce), ciphertext)
            .map_err(|_| EnvelopeError::DecryptFailed)
    }

    /// Returns the plaintext of `bytes`, passing legacy cleartext objects
    /// through untouched. This is what read paths should call so a store that
    /// predates encryption keeps working.
    pub fn decrypt_if_encrypted(&self, bytes: &[u8]) -> Result<Vec<u8>, EnvelopeError> {
        if !is_envelope(bytes) {
            return Ok(bytes.to_vec());
        }
        self.decrypt_envelope(bytes)
    }

    /// The X25519 secret backing key agreement. Private to this crate's
    /// envelope module and `Identity` itself.
    fn dh_secret(&self) -> StaticDhSecret {
        StaticDhSecret::from(crate::dh_secret_for(&self.keypair()))
    }
}

fn derive_wrap_key(
    shared: &[u8; 32],
    salt: &[u8],
    epk: &[u8; 32],
    recipient_pub: &[u8; 32],
) -> [u8; 32] {
    let mut info = Vec::with_capacity(WRAP_INFO.len() + EPK_LEN * 2);
    info.extend_from_slice(WRAP_INFO);
    info.extend_from_slice(epk);
    info.extend_from_slice(recipient_pub);

    let mut key = [0u8; KEY_LEN];
    Hkdf::<Sha256>::new(Some(salt), shared)
        .expand(&info, &mut key)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    key
}

fn random_bytes(len: usize) -> Result<[u8; CONTENT_NONCE_LEN], EnvelopeError> {
    let mut buf = [0u8; CONTENT_NONCE_LEN];
    if len != CONTENT_NONCE_LEN {
        return Err(EnvelopeError::Malformed("unsupported nonce length"));
    }
    OsRng.fill_bytes(&mut buf);
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing;
    use std::sync::Arc;

    fn identity(name: &str) -> Arc<Identity> {
        let path = std::env::temp_dir().join(format!(
            "canopee_envelope_test_{}_{}_{:?}",
            name,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        let id = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(Identity::create(path.to_str().unwrap()))
            .unwrap();
        Arc::new(id)
    }

    #[test]
    fn round_trips_for_the_owner() {
        let owner = identity("owner");
        let plaintext = b"the quick brown fox".repeat(50);
        let sealed = owner.encrypt_for_recipients(&plaintext, &[]).unwrap();
        assert_ne!(sealed, plaintext);
        assert!(is_envelope(&sealed));
        assert_eq!(owner.decrypt_envelope(&sealed).unwrap(), plaintext);
    }

    #[test]
    fn plaintext_never_appears_in_the_envelope() {
        let owner = identity("leak");
        let secret = b"SECRET-MARKER-STRING-0123456789".to_vec();
        let sealed = owner.encrypt_for_recipients(&secret, &[]).unwrap();
        assert!(
            !sealed.windows(secret.len()).any(|w| w == secret.as_slice()),
            "plaintext leaked into the ciphertext"
        );
    }

    #[test]
    fn an_explicit_recipient_can_decrypt() {
        let owner = identity("owner2");
        let friend = identity("friend");
        let plaintext = b"shared with a friend";
        let sealed = owner
            .encrypt_for_recipients(plaintext, &[friend.dh_public_key()])
            .unwrap();
        assert_eq!(friend.decrypt_envelope(&sealed).unwrap(), plaintext);
        // And the owner still can.
        assert_eq!(owner.decrypt_envelope(&sealed).unwrap(), plaintext);
    }

    #[test]
    fn a_stranger_cannot_decrypt() {
        let owner = identity("owner3");
        let friend = identity("friend3");
        let stranger = identity("stranger");
        let sealed = owner
            .encrypt_for_recipients(b"private", &[friend.dh_public_key()])
            .unwrap();
        assert!(matches!(
            stranger.decrypt_envelope(&sealed),
            Err(EnvelopeError::NoMatchingRecipient)
        ));
    }

    #[test]
    fn a_stranger_cannot_decrypt_even_without_an_explicit_recipient_list() {
        let owner = identity("owner4");
        let stranger = identity("stranger4");
        let sealed = owner.encrypt_for_recipients(b"private", &[]).unwrap();
        assert!(stranger.decrypt_envelope(&sealed).is_err());
    }

    /// The property the whole feature rests on: pairing copies the identity
    /// key, so a second device reads what the first device stored.
    #[test]
    fn a_paired_device_decrypts_what_the_source_device_stored() {
        let source = identity("source");
        let paired = pairing::paired_clone_for_test(&source);
        let sealed = source
            .encrypt_for_recipients(b"across devices", &[])
            .unwrap();
        assert_eq!(paired.decrypt_envelope(&sealed).unwrap(), b"across devices");
    }

    #[test]
    fn tampering_with_the_ciphertext_is_rejected() {
        let owner = identity("tamper");
        let mut sealed = owner.encrypt_for_recipients(b"authentic", &[]).unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01;
        assert!(matches!(
            owner.decrypt_envelope(&sealed),
            Err(EnvelopeError::DecryptFailed)
        ));
    }

    #[test]
    fn tampering_with_a_wrapped_key_is_rejected() {
        let owner = identity("tamper-wrap");
        let mut sealed = owner.encrypt_for_recipients(b"authentic", &[]).unwrap();
        // First wrapped content key's last byte (the AEAD tag).
        let idx = HEADER_LEN + PER_RECIPIENT_LEN - 1;
        sealed[idx] ^= 0x01;
        assert!(owner.decrypt_envelope(&sealed).is_err());
    }

    #[test]
    fn a_truncated_envelope_is_rejected_rather_than_panicking() {
        let owner = identity("truncate");
        let sealed = owner.encrypt_for_recipients(b"authentic", &[]).unwrap();
        for cut in [1usize, 8, HEADER_LEN, sealed.len() - 1] {
            let short = &sealed[..short_len(cut, sealed.len())];
            assert!(
                owner.decrypt_envelope(short).is_err(),
                "truncation to {cut} bytes should not decrypt"
            );
        }
    }

    fn short_len(cut: usize, len: usize) -> usize {
        cut.min(len.saturating_sub(1))
    }

    #[test]
    fn legacy_plaintext_passes_through() {
        let owner = identity("legacy");
        let legacy = b"an object stored before encryption existed";
        assert!(!is_envelope(legacy));
        assert_eq!(owner.decrypt_if_encrypted(legacy).unwrap(), legacy);
        assert!(matches!(
            owner.decrypt_envelope(legacy),
            Err(EnvelopeError::NotEncrypted)
        ));
    }

    #[test]
    fn an_unsupported_version_is_reported() {
        let owner = identity("version");
        let mut sealed = owner.encrypt_for_recipients(b"x", &[]).unwrap();
        sealed[4] = 99;
        assert!(matches!(
            owner.decrypt_envelope(&sealed),
            Err(EnvelopeError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn an_empty_payload_round_trips() {
        let owner = identity("empty");
        let sealed = owner.encrypt_for_recipients(b"", &[]).unwrap();
        assert_eq!(owner.decrypt_envelope(&sealed).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn every_envelope_uses_a_fresh_content_key_and_nonce() {
        let owner = identity("fresh");
        let a = owner
            .encrypt_for_recipients(b"same plaintext", &[])
            .unwrap();
        let b = owner
            .encrypt_for_recipients(b"same plaintext", &[])
            .unwrap();
        assert_ne!(a, b, "two envelopes of identical plaintext must differ");
        // …and both still read back correctly.
        assert_eq!(owner.decrypt_envelope(&a).unwrap(), b"same plaintext");
        assert_eq!(owner.decrypt_envelope(&b).unwrap(), b"same plaintext");
    }

    #[test]
    fn wrap_keys_are_domain_separated_from_the_raw_agreement() {
        // The raw DH output must never be usable as the wrapping key directly.
        let owner = identity("domain");
        let friend = identity("domain-friend");
        let shared_raw = owner.agree(&friend.dh_public_key());
        let salt = [7u8; SALT_LEN];
        let epk = [3u8; EPK_LEN];
        let derived = derive_wrap_key(&shared_raw, &salt, &epk, &friend.dh_public_key());
        assert_ne!(derived, shared_raw);
    }
}
