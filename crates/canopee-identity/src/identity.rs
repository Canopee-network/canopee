use anyhow::Result;
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, OsRng};
use chacha20poly1305::{AeadCore, KeyInit, XChaCha20Poly1305, XNonce};
use libp2p::identity::{Keypair, PeerId};
use rand_core::RngCore;
use serde::{Deserialize, Serialize};
use tokio::fs;
use x25519_dalek::{PublicKey as DhPublicKey, StaticSecret as DhSecret};

/// Domain-separates the X25519 key agreement secret derived from this
/// node's Ed25519 signing key from any other secret `Keypair::derive_secret`
/// might be asked to produce — changing this string changes every derived
/// DH keypair network-wide, so treat it as part of the protocol, not a
/// tunable.
const DH_DOMAIN: &[u8] = b"canopee/dh/x25519/v1";

/// Byte at the start of an encrypted-at-rest identity file. Anything else at
/// the head of the file is treated as a legacy plaintext protobuf keypair.
/// Changing any of these framing bytes breaks decryption of existing encrypted
/// identity files network-wide, so treat them as part of the protocol.
const ENCRYPTED_MAGIC: &[u8] = b"canopee-v1-ek";
/// Identifies the KDF + AEAD construction used. Parsed leniently on read; only
/// v1 is produced today, so we can add a v2 later without breaking decryption
/// of existing files (each version keeps a distinct tag).
const ENCRYPTED_VERSION: &[u8] = b"argon2id/xchacha20";
/// KDF output / XChaCha20 key length in bytes (256 bits).
const KEY_LEN: usize = 32;
/// Argon2id salt length in bytes.
const SALT_LEN: usize = 16;
/// Randomized 192-bit nonce length in bytes; never reused across writes.
const NONCE_LEN: usize = 24;

#[allow(unused)]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdentityId(String);

impl IdentityId {
    /// Builds an `IdentityId` from a previously-displayed
    /// `canopee://identity/<peer-id>` string, e.g. one shared out of band by
    /// another user. Does not validate the peer id is well-formed.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for IdentityId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[allow(unused)]
pub struct Identity {
    signing_key: Keypair,
    // Derived from `signing_key` via `derive_secret`, not independently
    // generated or persisted — reloading the same signing key always
    // reproduces the same DH keypair. Never sign with this, and never use
    // `signing_key` for key agreement: the two must stay on separate curves
    // and separate purposes even though they originate from the same seed.
    dh_secret: DhSecret,
    pub identity_id: IdentityId,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("identity_id", &self.identity_id)
            .finish_non_exhaustive()
    }
}

impl Identity {
    pub fn id(&self) -> &IdentityId {
        &self.identity_id
    }

    pub fn sign(&self, data: &[u8]) -> Result<Vec<u8>> {
        Ok(self.signing_key.sign(data)?)
    }

    pub fn verify(&self, data: &[u8], signature: &[u8]) -> bool {
        self.signing_key.public().verify(data, signature)
    }

    pub fn public_key_bytes(&self) -> Vec<u8> {
        self.signing_key.public().encode_protobuf()
    }

    pub fn keypair(&self) -> Keypair {
        self.signing_key.clone()
    }

    /// This identity's X25519 public key, for key agreement — publish this
    /// (e.g. alongside `public_key_bytes()`) so others can `agree` with it.
    pub fn dh_public_key(&self) -> [u8; 32] {
        DhPublicKey::from(&self.dh_secret).to_bytes()
    }

    /// Diffie-Hellman key agreement against another identity's
    /// `dh_public_key()`, producing a shared secret only the two of them can
    /// derive. This is a raw primitive: callers are expected to run the
    /// result through a KDF (and manage ratcheting/session state themselves)
    /// rather than using it directly as a cipher key.
    pub fn agree(&self, their_dh_public_key: &[u8; 32]) -> [u8; 32] {
        self.dh_secret
            .diffie_hellman(&DhPublicKey::from(*their_dh_public_key))
            .to_bytes()
    }

    pub async fn create(path: &str) -> Result<Self> {
        let signing_key = Keypair::generate_ed25519();
        let bytes = signing_key.to_protobuf_encoding()?;
        fs::write(path, bytes).await?;

        Ok(Self::from_keypair(signing_key))
    }

    /// Loads the identity at `path`, creating a fresh one only if none exists.
    ///
    /// Exists for shared-user-root configurations where several apps may
    /// race to be first: the key file is created with `create_new`, so at
    /// most one writer wins and every loser ends up loading (after a retry
    /// window, since the winner may still be flushing bytes) the same key.
    /// This is the "one identity per person" guarantee — no app can silently
    /// invent a second identity for the same user root.
    pub async fn create_if_absent(path: &str) -> Result<Self> {
        use std::io::ErrorKind;
        use tokio::io::AsyncWriteExt;

        let signing_key = Keypair::generate_ed25519();
        let bytes = signing_key.to_protobuf_encoding()?;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await
        {
            Ok(mut file) => {
                file.write_all(&bytes).await?;
                file.sync_all().await?;
                Ok(Self::from_keypair(signing_key))
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                // Another process/thread created the key while we were racing.
                // Give the winner a moment to finish writing, then adopt the
                // same identity. Each failed read is "not ready yet" — the
                // file exists before its bytes are flushed.
                for attempt in 0..25 {
                    match Self::load(path).await {
                        Ok(identity) => return Ok(identity),
                        Err(_) if attempt < 24 => {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await
                        }
                        Err(e) => return Err(e),
                    }
                }
                unreachable!()
            }
            Err(e) => Err(e.into()),
        }
    }

    pub async fn load(path: &str) -> Result<Self> {
        let bytes = fs::read(path).await?;
        let signing_key = Keypair::from_protobuf_encoding(&bytes)
            .map_err(|e| anyhow::anyhow!("invalid keypair file: {e}"))?;
        Ok(Self::from_keypair(signing_key))
    }

    /// Create a new identity whose private key is written to `path`
    /// encrypted at rest (Argon2id-derived key + XChaCha20-Poly1305), so the
    /// file alone is not enough to reconstruct the signing key. `passphrase`
    /// must be supplied again on every later [`Self::load`] via
    /// [`Self::load_encrypted`]; there is no recoverable forget-this-passphrase.
    pub async fn create_encrypted(path: &str, passphrase: &str) -> Result<Self> {
        let signing_key = Keypair::generate_ed25519();
        let plaintext = signing_key.to_protobuf_encoding()?;
        let encrypted = encrypt_key_envelope(&plaintext, passphrase.as_bytes())?;
        fs::write(path, encrypted).await?;

        Ok(Self::from_keypair(signing_key))
    }

    /// Load an identity from `path`, decrypting if the file is an
    /// encrypted-at-rest envelope or falling back to the legacy plaintext
    /// protobuf format. Returns `(identity, encrypted_at_rest)` so callers can
    /// tell whether the key on disk is encrypted. Fails if the file is an
    /// envelope but `passphrase` is wrong or the file has been tampered with.
    pub async fn load_encrypted(path: &str, passphrase: &str) -> Result<(Self, bool)> {
        let bytes = fs::read(path).await?;
        if is_encrypted_envelope(&bytes) {
            let plaintext = decrypt_key_envelope(&bytes, passphrase.as_bytes())?;
            let signing_key = Keypair::from_protobuf_encoding(&plaintext)
                .map_err(|e| anyhow::anyhow!("decrypted identity is not a valid keypair: {e}"))?;
            Ok((Self::from_keypair(signing_key), true))
        } else {
            Ok((Self::load(path).await?, false))
        }
    }

    /// The raw protobuf-encoded private key bytes — the same bytes
    /// [`Self::create`] writes to disk — for transfer to another device.
    pub fn export_bytes(&self) -> Result<Vec<u8>> {
        Ok(self.signing_key.to_protobuf_encoding()?)
    }

    /// Serializes this identity's private key into a transferable, encrypted
    /// envelope — the same Argon2id + XChaCha20-Poly1305 construction used
    /// for encrypted-at-rest files, so an exported key benefits from the same
    /// KDF + AEAD guarantees while in transit. `passphrase` must be supplied
    /// again on import; there is no recoverable-forget-this-passphrase.
    pub fn export_encrypted(&self, passphrase: &str) -> Result<Vec<u8>> {
        let plaintext = self.export_bytes()?;
        encrypt_key_envelope(&plaintext, passphrase.as_bytes())
    }

    /// Imports a private key from an [`Self::export_encrypted`] envelope,
    /// validating the passphrase and the envelope's integrity before use.
    /// Returns the identity in memory without touching disk — the caller
    /// decides when (and whether) it is persisted.
    pub fn import_from_encrypted(bytes: &[u8], passphrase: &str) -> Result<Self> {
        let plaintext = decrypt_key_envelope(bytes, passphrase.as_bytes())?;
        let signing_key = Keypair::from_protobuf_encoding(&plaintext)
            .map_err(|e| anyhow::anyhow!("decrypted identity is not a valid keypair: {e}"))?;
        Ok(Self::from_keypair(signing_key))
    }

    /// Parses a private key from raw protobuf bytes (as produced by
    /// [`Self::export_bytes`]) back into an `Identity` in memory, without
    /// touching disk. Used by the LAN pairing flow, where the identity key
    /// crosses over the encrypted pairing channel.
    pub fn import_bytes(bytes: &[u8]) -> Result<Self> {
        let signing_key = Keypair::from_protobuf_encoding(bytes)
            .map_err(|e| anyhow::anyhow!("invalid identity keypair bytes: {e}"))?;
        Ok(Self::from_keypair(signing_key))
    }

    fn from_keypair(signing_key: Keypair) -> Self {
        let peer_id = PeerId::from(signing_key.public());
        let identity_id = format!("canopee://identity/{}", peer_id);
        let dh_secret = derive_dh_secret(&signing_key);

        Self {
            signing_key,
            dh_secret,
            identity_id: IdentityId(identity_id),
        }
    }
}

/// Scratch-free in-memory framing of one encrypted-at-rest identity file.
///
/// Layout (all multi-byte integers little-endian):
///   `ENCRYPTED_MAGIC` (12 bytes)
///   `ENCRYPTED_VERSION` (18 bytes, ASCII)
///   argon2 memory cost      u32
///   argon2 time cost        u32
///   argon2 parallelism      u32
///   salt                    [u8; 16]
///   nonce                   [u8; 24]
///   ciphertext || tag       (rest of file; keypair protobuf + 16-byte Poly1305 tag)
const HEADER_LEN: usize = ENCRYPTED_MAGIC.len() + ENCRYPTED_VERSION.len() + 3 * 4 + SALT_LEN + NONCE_LEN;

fn is_encrypted_envelope(bytes: &[u8]) -> bool {
    bytes.len() > HEADER_LEN && bytes[..ENCRYPTED_MAGIC.len()] == *ENCRYPTED_MAGIC
}

/// Derive the 256-bit XChaCha20 key from a passphrase + envelope salt.
/// `params` is stored in the envelope so the same passphrase can decrypt
/// after (e.g.) an OWASP-recommended parameter bump, but the construction must
/// stay Argon2id for older files.
fn derive_wrap_key(passphrase: &[u8], salt: &[u8], params: &Params) -> [u8; KEY_LEN] {
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params.clone());
    let mut key = [0u8; KEY_LEN];
    argon2
        .hash_password_into(passphrase, salt, &mut key)
        .expect("argon2 params are valid");
    key
}

fn encrypt_key_envelope(plaintext: &[u8], passphrase: &[u8]) -> Result<Vec<u8>> {
    let params = Params::default();
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);

    let key = derive_wrap_key(passphrase, &salt, &params);
    let cipher = XChaCha20Poly1305::new((&key).into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);

    let mut envelope = Vec::with_capacity(HEADER_LEN + plaintext.len() + 16);
    envelope.extend_from_slice(ENCRYPTED_MAGIC);
    envelope.extend_from_slice(ENCRYPTED_VERSION);
    envelope.extend_from_slice(&params.m_cost().to_le_bytes());
    envelope.extend_from_slice(&params.t_cost().to_le_bytes());
    envelope.extend_from_slice(&params.p_cost().to_le_bytes());
    envelope.extend_from_slice(&salt);
    envelope.extend_from_slice(&nonce);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("encryption failed: {e}"))?;
    envelope.extend_from_slice(&ciphertext);
    Ok(envelope)
}

fn decrypt_key_envelope(bytes: &[u8], passphrase: &[u8]) -> Result<Vec<u8>> {
    if !is_encrypted_envelope(bytes) {
        anyhow::bail!("not an encrypted-at-rest identity envelope");
    }
    let version_end = ENCRYPTED_MAGIC.len() + ENCRYPTED_VERSION.len();
    let version = &bytes[ENCRYPTED_MAGIC.len()..version_end];
    if version != ENCRYPTED_VERSION {
        anyhow::bail!(
            "unsupported encrypted-at-rest version {:?}",
            String::from_utf8_lossy(version)
        );
    }

    let mut off = version_end;
    let m_cost = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
    let t_cost = u32::from_le_bytes(bytes[off + 4..off + 8].try_into().unwrap());
    let p_cost = u32::from_le_bytes(bytes[off + 8..off + 12].try_into().unwrap());
    off += 12;
    let salt = &bytes[off..off + SALT_LEN];
    off += SALT_LEN;
    let nonce = &bytes[off..off + NONCE_LEN];
    off += NONCE_LEN;
    let ciphertext = &bytes[off..];

    let params = Params::new(m_cost, t_cost, p_cost, Some(KEY_LEN))
        .map_err(|e| anyhow::anyhow!("identity envelope declares invalid argon2 params: {e}"))?;
    let key = derive_wrap_key(passphrase, salt, &params);
    let cipher = XChaCha20Poly1305::new((&key).into());
    let nonce = XNonce::from_slice(nonce);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("wrong passphrase or tampered encrypted identity file"))
}

fn derive_dh_secret(signing_key: &Keypair) -> DhSecret {
    let seed = signing_key
        .derive_secret(DH_DOMAIN)
        .expect("derive_secret is supported for ed25519 keys");
    DhSecret::from(seed)
}

#[tokio::test]
async fn create_if_absent_makes_one_identity_per_path() {
    let path = "./create_if_absent.key";
    let _ = std::fs::remove_file(path);

    let first = Identity::create_if_absent(path).await.unwrap();
    assert_eq!(first.id().to_string(), format!("canopee://identity/{}", first.keypair().public().to_peer_id()));

    // A second call must adopt the existing key, never mint a new one.
    let second = Identity::create_if_absent(path).await.unwrap();
    assert_eq!(first.id(), second.id());
    assert_eq!(first.dh_public_key(), second.dh_public_key());
}

#[tokio::test]
async fn create_if_absent_survives_concurrent_first_run() {
    let path = "./create_if_absent_race.key";
    let _ = std::fs::remove_file(path);

    let mut handles = Vec::new();
    for _ in 0..8 {
        let path = path.to_string();
        handles.push(tokio::spawn(async move {
            Identity::create_if_absent(&path).await.unwrap().id().clone()
        }));
    }
    let mut ids: Vec<_> = Vec::new();
    for h in handles {
        ids.push(h.await.unwrap());
    }
    ids.dedup();
    assert_eq!(ids.len(), 1, "all racers must converge on one identity");
}

#[tokio::test]
async fn identity_can_sign_and_verify() {
    let identity = Identity::create("./test.key").await.unwrap();
    let message = b"hello canopee";
    let signature = identity.sign(message).unwrap();

    assert!(identity.verify(message, &signature));
}

#[tokio::test]
async fn dh_agreement_is_symmetric() {
    let alice = Identity::create("./alice_dh_test.key").await.unwrap();
    let bob = Identity::create("./bob_dh_test.key").await.unwrap();

    let shared_from_alice = alice.agree(&bob.dh_public_key());
    let shared_from_bob = bob.agree(&alice.dh_public_key());

    assert_eq!(shared_from_alice, shared_from_bob);
}

#[tokio::test]
async fn dh_public_key_is_deterministic_across_load() {
    let identity = Identity::create("./reload_dh_test.key").await.unwrap();
    let dh_public_key = identity.dh_public_key();

    let reloaded = Identity::load("./reload_dh_test.key").await.unwrap();

    assert_eq!(dh_public_key, reloaded.dh_public_key());
}

#[tokio::test]
async fn encrypted_identity_round_trips_through_disk() {
    let passphrase = "correct horse battery staple";
    let identity = Identity::create_encrypted("./enc_roundtrip.key", passphrase)
        .await
        .unwrap();
    let id = identity.id().clone();
    let dh = identity.dh_public_key();

    let (loaded, encrypted_at_rest) =
        Identity::load_encrypted("./enc_roundtrip.key", passphrase).await.unwrap();

    assert!(encrypted_at_rest, "file should be detected as encrypted-at-rest");
    assert_eq!(loaded.id(), &id);
    assert_eq!(loaded.dh_public_key(), dh);
}

#[tokio::test]
async fn encrypted_identity_is_not_a_plaintext_keypair() {
    let passphrase = "pw";
    let identity = Identity::create_encrypted("./enc_plaintext.key", passphrase)
        .await
        .unwrap();
    let bytes = std::fs::read("./enc_plaintext.key").unwrap();

    assert!(
        Keypair::from_protobuf_encoding(&bytes).is_err(),
        "encrypted-at-rest file must not be loadable as raw protobuf"
    );
    // The RHS is only used to exercise the constructor's happy path in a way
    // that won't trip the "unused" allow gate.
    let _ = identity.id().clone();
}

#[tokio::test]
async fn encrypted_identity_rejects_wrong_passphrase() {
    let passphrase = "right";
    let identity = Identity::create_encrypted("./enc_wrong_pw.key", passphrase)
        .await
        .unwrap();

    let result = Identity::load_encrypted("./enc_wrong_pw.key", "wrong").await;

    assert!(result.is_err(), "wrong passphrase must fail to decrypt");
    let _ = identity.id().clone();
}

#[tokio::test]
async fn load_encrypted_falls_back_to_plaintext_file() {
    let plain = Identity::create("./enc_fallback.key").await.unwrap();
    let id = plain.id().clone();

    let (loaded, encrypted_at_rest) =
        Identity::load_encrypted("./enc_fallback.key", "irrelevant").await.unwrap();

    assert!(!encrypted_at_rest, "plaintext identity must not be flagged encrypted");
    assert_eq!(loaded.id(), &id);
}

#[tokio::test]
async fn encrypted_key_material_differs_from_plaintext() {
    let passphrase = "pw";
    let enc = Identity::create_encrypted("./enc_diff.key", passphrase)
        .await
        .unwrap();
    let plain = Identity::create("./enc_diff_plain.key").await.unwrap();
    let enc_bytes = std::fs::read("./enc_diff.key").unwrap();
    let plain_bytes = std::fs::read("./enc_diff_plain.key").unwrap();

    assert_ne!(enc_bytes, plain_bytes, "encrypted file must not equal plaintext bytes");

    // The Identity debug output must not reveal the signing key material.
    let debug_enc = format!("{enc:?}");
    assert!(!debug_enc.contains("signing_key"), "Debug must redact the signing key");
    let _ = plain.keypair();
}

#[tokio::test]
async fn exported_identity_round_trips_through_encrypted_bytes() {
    let identity = Identity::create("./export_roundtrip.key").await.unwrap();
    let id = identity.id().clone();
    let dh = identity.dh_public_key();

    let bytes = identity.export_encrypted("transfer secret").unwrap();
    assert_ne!(
        bytes,
        identity.export_bytes().unwrap(),
        "encrypted export must not equal plaintext key bytes"
    );

    let imported = Identity::import_from_encrypted(&bytes, "transfer secret").unwrap();
    assert_eq!(imported.id(), &id);
    assert_eq!(imported.dh_public_key(), dh);
    assert_eq!(imported.export_bytes().unwrap(), identity.export_bytes().unwrap());
}

#[tokio::test]
async fn exported_encrypted_identity_rejects_wrong_passphrase() {
    let identity = Identity::create("./export_wrong_pw.key").await.unwrap();
    let bytes = identity.export_encrypted("right").unwrap();

    let bad = Identity::import_from_encrypted(&bytes, "wrong");
    assert!(bad.is_err(), "wrong passphrase must fail to import");
}

#[test]
fn exported_encrypted_identity_rejects_garbage() {
    let result = Identity::import_from_encrypted(b"not an envelope at all", "pw");
    assert!(result.is_err(), "garbage must fail to import");
}
