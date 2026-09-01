//! The E2E-encryption core of the chat app (tutorial steps 6b/6c).
//!
//! Everything in here is pure and unit-testable without a GUI or a running
//! node: contact-string parsing, deterministic per-conversation topic
//! derivation, HKDF conversation-key derivation from an X25519 agreement,
//! and ChaCha20-Poly1305 encrypt/decrypt with per-message random nonces.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

use canopee_identity::Identity;

/// Domain separation for HKDF, so this derivation can never be confused with
/// any other use of the same X25519 shared secret.
const KDF_DOMAIN: &[u8] = b"canopee-chat/conversation-key/v1";
/// Domain separation for the per-conversation topic hash.
const TOPIC_DOMAIN: &[u8] = b"canopee-chat/topic/v1";

/// A contact as shared out of band (tutorial step 3): a `canopee://identity`
/// string that also carries the contact's X25519 public key, so both sides
/// can derive the conversation key without any DHT lookup (the minimal form
/// of step 6a).
#[derive(Debug, Clone, PartialEq)]
pub struct Contact {
    /// User-chosen display name.
    pub name: String,
    /// The libp2p peer id, e.g. `12D3KooW...` (informational; used to share
    /// multiaddrs and to filter our own gossipsub echoes).
    pub peer_id: String,
    /// The contact's X25519 public key (`Identity::dh_public_key`).
    pub dh_public_key: [u8; 32],
}

impl Contact {
    /// Converts to the shared user-store contact (`canopee-storage`), so the
    /// app can persist its address book in the user's shared `ContactList`
    /// object rather than a private per-app list.
    pub fn to_user_contact(&self) -> canopee_storage::Contact {
        canopee_storage::Contact {
            name: self.name.clone(),
            peer_id: self.peer_id.clone(),
            dh_public_key: self.dh_public_key,
            note: None,
        }
    }
}

impl From<canopee_storage::Contact> for Contact {
    fn from(c: canopee_storage::Contact) -> Self {
        Self {
            name: c.name,
            peer_id: c.peer_id,
            dh_public_key: c.dh_public_key,
        }
    }
}

/// Formats a full contact string: `canopee://identity/<peer-id>#dh=<hex>`.
pub fn format_contact(peer_id: &str, dh_public_key: &[u8; 32]) -> String {
    format!(
        "canopee://identity/{}#dh={}",
        peer_id,
        hex::encode(dh_public_key)
    )
}

/// Parses a contact string as produced by [`format_contact`], returning the
/// contact's X25519 public key.
pub fn parse_contact(s: &str) -> Result<[u8; 32], String> {
    let (_, dh_part) = s
        .split_once('#')
        .ok_or_else(|| "contact string is missing the #dh= fragment".to_string())?;
    let dh_hex = dh_part
        .strip_prefix("dh=")
        .ok_or_else(|| "expected #dh=<hex> fragment".to_string())?;
    if dh_hex.len() != 64 {
        return Err("DH public key must be exactly 32 bytes (64 hex chars)".to_string());
    }
    let bytes = hex::decode(dh_hex).map_err(|e| format!("invalid hex in DH key: {e}"))?;
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// Deterministic per-conversation gossipsub topic, derived from both
/// participants' DH public keys sorted so both sides compute the same string.
///
/// An unguessable topic is defense-in-depth: step 6c's encryption is the
/// real message privacy; the topic just stops the string itself from being
/// guessable (tutorial Step 4's design question).
pub fn conversation_topic(our: &[u8; 32], their: &[u8; 32]) -> String {
    let (a, b) = if our < their { (our, their) } else { (their, our) };
    let mut hasher = Sha256::new();
    hasher.update(TOPIC_DOMAIN);
    hasher.update(a);
    hasher.update(b);
    hex::encode(hasher.finalize())
}

/// One 1:1 conversation: a contact plus the derived session material.
///
/// `conversation_key` is deterministic for a given pair of identities, so it
/// is derived once at construction and cached in memory (tutorial step 6b's
/// design question). Forward secrecy is explicitly out of scope (see the
/// tutorial's stretch note).
#[derive(Debug, Clone)]
pub struct Conversation {
    pub contact: Contact,
    topic: String,
    conversation_key: [u8; 32],
}

impl Conversation {
    pub fn new(identity: &Identity, contact: Contact) -> Self {
        let our_dh = identity.dh_public_key();
        let topic = conversation_topic(&our_dh, &contact.dh_public_key);
        // X25519 agreement; symmetric on both sides.
        let shared_secret = identity.agree(&contact.dh_public_key);
        let mut conversation_key = [0u8; 32];
        Hkdf::<Sha256>::new(None, &shared_secret)
            .expand(KDF_DOMAIN, &mut conversation_key)
            .expect("32 bytes is a valid HKDF output length");
        Self {
            contact,
            topic,
            conversation_key,
        }
    }

    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Encrypts a plaintext message: fresh random 12-byte nonce, then
    /// ChaCha20-Poly1305. Output is `nonce || ciphertext` (nonce need not be
    /// secret, only unique). Returning `Result` rather than panicking keeps
    /// `send_message` able to fail cleanly.
    pub fn encrypt(&self, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.conversation_key));
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), plaintext)
            .map_err(|e| anyhow::anyhow!("encryption failed: {e:?}"))?;
        let mut out = nonce_bytes.to_vec();
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Inverse of [`Conversation::encrypt`]. Returns `None` on any failure
    /// (wrong key, truncated payload, tampered ciphertext) — a message we
    /// can't decrypt is dropped, never displayed.
    pub fn decrypt(&self, payload: &[u8]) -> Option<Vec<u8>> {
        if payload.len() < 12 {
            return None;
        }
        let (nonce, ciphertext) = payload.split_at(12);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.conversation_key));
        cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Creates a fresh, ephemeral identity for testing. Each call gets its
    /// own key file (unique per test process), so "alice", "bob" and
    /// "mallory" in a single test are genuinely *different* identities —
    /// sharing one file would make every participant the same person and the
    /// crypto assertions below vacuous.
    async fn test_identity() -> Identity {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "canopee_chat_test_identity_{}_{}",
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Identity::create(dir.join("identity.key").to_str().unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn contact_string_roundtrip() {
        let identity = test_identity().await;
        let peer_id = identity.keypair().public().to_peer_id().to_base58();
        let dh = identity.dh_public_key();
        let s = format_contact(&peer_id, &dh);
        assert!(s.starts_with("canopee://identity/"));
        assert!(s.ends_with(&format!("#dh={}", hex::encode(dh))));
        assert_eq!(parse_contact(&s).unwrap(), dh);
    }

    #[test]
    fn parse_contact_rejects_bad_input() {
        assert!(parse_contact("no-fragment").is_err());
        assert!(parse_contact("canopee://identity/x#key=abcdef").is_err());
        assert!(parse_contact("canopee://identity/x#dh=abcd").is_err());
        assert!(parse_contact("canopee://identity/x#dh=zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_err());
    }

    #[tokio::test]
    async fn both_sides_derive_same_key_and_topic() {
        let alice = test_identity().await;
        let bob = test_identity().await;

        let alice_dh = alice.dh_public_key();
        let bob_dh = bob.dh_public_key();

        let alice_conv = Conversation::new(
            &alice,
            Contact {
                name: "bob".to_string(),
                peer_id: bob.keypair().public().to_peer_id().to_base58(),
                dh_public_key: bob_dh,
            },
        );
        let bob_conv = Conversation::new(
            &bob,
            Contact {
                name: "alice".to_string(),
                peer_id: alice.keypair().public().to_peer_id().to_base58(),
                dh_public_key: alice_dh,
            },
        );

        assert_eq!(alice_conv.topic(), bob_conv.topic());
        assert_eq!(
            alice_conv.conversation_key, bob_conv.conversation_key,
            "X25519 agreement must be symmetric"
        );
        assert_eq!(
            conversation_topic(&alice_dh, &bob_dh),
            conversation_topic(&bob_dh, &alice_dh),
            "topic derivation must be order-independent"
        );
    }

    #[tokio::test]
    async fn conversation_key_is_domain_separated_from_raw_agreement() {
        let alice = test_identity().await;
        let bob = test_identity().await;
        let raw = alice.agree(&bob.dh_public_key());
        let conv = Conversation::new(
            &alice,
            Contact {
                name: "bob".to_string(),
                peer_id: String::new(),
                dh_public_key: bob.dh_public_key(),
            },
        );
        assert_ne!(
            conv.conversation_key, raw,
            "HKDF derivation must differ from the raw agree() output"
        );
        let mut diff = [0u8; 32];
        Hkdf::<Sha256>::new(None, &raw)
            .expand(b"canopee-chat/topic/v1", &mut diff)
            .unwrap();
        assert_ne!(
            conv.conversation_key, diff,
            "changing the domain string must change the derived key"
        );
    }

    #[tokio::test]
    async fn encrypt_decrypt_roundtrip_between_peers() {
        let alice = test_identity().await;
        let bob = test_identity().await;

        let alice_conv = Conversation::new(
            &alice,
            Contact {
                name: "bob".to_string(),
                peer_id: bob.keypair().public().to_peer_id().to_base58(),
                dh_public_key: bob.dh_public_key(),
            },
        );
        let bob_conv = Conversation::new(
            &bob,
            Contact {
                name: "alice".to_string(),
                peer_id: alice.keypair().public().to_peer_id().to_base58(),
                dh_public_key: alice.dh_public_key(),
            },
        );

        let msg = b"hello bob, this is secret";
        let wire = alice_conv.encrypt(msg).unwrap();
        assert!(
            !wire.windows(msg.len()).any(|w| w == msg),
            "plaintext must not appear in what crosses the network"
        );
        assert_eq!(bob_conv.decrypt(&wire).unwrap(), msg);
    }

    #[tokio::test]
    async fn decrypt_fails_with_wrong_key_and_when_tampered() {
        let alice = test_identity().await;
        let bob = test_identity().await;
        let mallory = test_identity().await;

        let alice_conv = Conversation::new(
            &alice,
            Contact {
                name: "bob".to_string(),
                peer_id: bob.keypair().public().to_peer_id().to_base58(),
                dh_public_key: bob.dh_public_key(),
            },
        );
        let mallory_conv = Conversation::new(
            &mallory,
            Contact {
                name: "bob".to_string(),
                peer_id: String::new(),
                dh_public_key: bob.dh_public_key(),
            },
        );

        let wire = alice_conv.encrypt(b"secret").unwrap();
        assert!(
            mallory_conv.decrypt(&wire).is_none(),
            "a different identity must not be able to decrypt"
        );

        let mut tampered = wire.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert!(
            alice_conv.decrypt(&tampered).is_none(),
            "tampered ciphertext must fail the AEAD tag check"
        );

        assert!(
            alice_conv.decrypt(b"short").is_none(),
            "payload shorter than a nonce must be rejected"
        );
    }

    #[test]
    fn topic_differs_between_different_contacts() {
        let our = [7u8; 32];
        let a = [1u8; 32];
        let b = [2u8; 32];
        assert_ne!(conversation_topic(&our, &a), conversation_topic(&our, &b));
    }
}