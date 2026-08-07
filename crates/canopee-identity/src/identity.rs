use anyhow::Result;
use libp2p::identity::{Keypair, PeerId};
use serde::{Deserialize, Serialize};
use tokio::fs;
use x25519_dalek::{PublicKey as DhPublicKey, StaticSecret as DhSecret};

/// Domain-separates the X25519 key agreement secret derived from this
/// node's Ed25519 signing key from any other secret `Keypair::derive_secret`
/// might be asked to produce — changing this string changes every derived
/// DH keypair network-wide, so treat it as part of the protocol, not a
/// tunable.
const DH_DOMAIN: &[u8] = b"canopee/dh/x25519/v1";

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
        let peer_id = PeerId::from(signing_key.public());
        let identity_id = format!("canopee://identity/{}", peer_id);
        let dh_secret = derive_dh_secret(&signing_key);
        let bytes = signing_key.to_protobuf_encoding()?;
        fs::write(path, bytes).await?;

        Ok(Self {
            signing_key,
            dh_secret,
            identity_id: IdentityId(identity_id),
        })
    }

    pub async fn load(path: &str) -> Result<Self> {
        let bytes = fs::read(path).await?;
        let signing_key = Keypair::from_protobuf_encoding(&bytes).expect("invalid keypair file");
        let peer_id = PeerId::from(signing_key.public());
        let identity_id = format!("canopee://identity/{}", peer_id);
        let dh_secret = derive_dh_secret(&signing_key);

        Ok(Self {
            signing_key,
            dh_secret,
            identity_id: IdentityId(identity_id),
        })
    }
}

fn derive_dh_secret(signing_key: &Keypair) -> DhSecret {
    let seed = signing_key
        .derive_secret(DH_DOMAIN)
        .expect("derive_secret is supported for ed25519 keys");
    DhSecret::from(seed)
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
