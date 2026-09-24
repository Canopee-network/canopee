use canopee_identity::{Identity, IdentityId};
use canopee_protocol::PairingPayload;
use canopee_storage::{ExportBundle, ObjectId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ObjectRequest {
    /// Pull: ask a peer for an object it holds. Served from the node's
    /// `ObjectProvider` (shared/cached objects only).
    GetObject(ObjectId),
    /// Push: hand a peer an `ExportBundle` to store, so a freshly created
    /// object lands in already-connected peers' stores *without* a DHT
    /// provider round-trip. The receiver verifies the signature
    /// (`put_verified`) before persisting; see [`ObjectResponse::Stored`].
    Store(ExportBundle),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ObjectResponse {
    Object(ExportBundle),
    NotFound,
    /// The receiver verified and stored a pushed [`ObjectRequest::Store`]
    /// bundle in its own store.
    Stored,
    /// The receiver refused a pushed bundle (invalid signature, unwritable
    /// store, ...). The message describes why.
    StoreFailed(String),
}

/// The LAN device-pairing request: the source device (existing identity)
/// delivers the AEAD-encrypted identity + records to the new device.
///
/// The pairing code itself is never transmitted — `accept_pairing` looks up
/// the session it minted (`session_id`), derives the key from the code it kept
/// in memory, and decrypts. `from`/`device_id` are both device peer ids and
/// are mixed into the KDF salt so the session key is bound to the two devices.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CanopeePairingRequest {
    /// Device peer id of the *sending* (source) device.
    pub from: String,
    /// Device peer id of the *receiving* (new) device.
    pub device_id: String,
    pub session_id: String,
    pub payload: PairingPayload,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum CanopeePairingResponse {
    /// Accepted, with a human-readable status message.
    Accepted(String),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct PubSubMessage {
    pub topic: String,
    pub source: Option<libp2p::PeerId>,
    pub data: Vec<u8>,
}

/// The only request headers an edge forwards to a publisher — exactly what
/// the static-file logic in `canopee-runtime` reads (content-negotiation,
/// byte ranges, conditional revalidation). Anything else is private to the
/// HTTP client and dropped at the edge.
pub const SERVE_FORWARDED_HEADERS: &[&str] = &["accept-encoding", "range", "if-none-match"];

/// One HTTP request forwarded by a Canopee edge to the publisher whose
/// connection carries the requested `username` subdomain. `path` is the full
/// request target (`/portfolio/about`); the publisher splits off the first
/// segment as the app name and serves the rest. Only the headers listed in
/// [`SERVE_FORWARDED_HEADERS`] are present.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ServeRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl ServeRequest {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// The publisher's answer to a [`ServeRequest`]: a ready-to-write HTTP
/// response (status line + headers + body), which the edge relays verbatim to
/// the original client.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ServeResponse {
    /// `"200 OK"`, `"404 Not Found"`, `"304 Not Modified"`, ...
    pub status: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// A signed claim by a publisher node that *this device's connection* is
/// authorized to serve the app whose manifest is `app_id` on an edge. The
/// edge verifies the signature against `identity`, fetches the manifest from
/// the registering peer, and checks the manifest's `owner` is `identity` —
/// since the app id is a hash of the manifest (which embeds the owner),
/// nobody can register someone else's app. `timestamp` must be fresh.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ServeRegistration {
    pub app_id: String,
    pub identity: IdentityId,
    pub public_key: Vec<u8>,
    pub timestamp: u64,
    pub signature: Vec<u8>,
}

impl ServeRegistration {
    /// Signs a fresh registration for `app_id` (a manifest object id) with
    /// `identity`.
    pub fn sign(identity: &Identity, app_id: &str, timestamp: u64) -> anyhow::Result<Self> {
        let identity_id = identity.id().clone();
        let signature = identity.sign(&Self::signing_bytes(app_id, &identity_id, timestamp))?;
        Ok(Self {
            app_id: app_id.to_string(),
            identity: identity_id,
            public_key: identity.public_key_bytes(),
            timestamp,
            signature,
        })
    }

    fn signing_bytes(app_id: &str, identity: &IdentityId, timestamp: u64) -> Vec<u8> {
        format!("canopee-serve-registration:v2:{app_id}:{identity}:{timestamp}").into_bytes()
    }

    /// Verifies the signature was produced by the registered identity's key.
    /// Does NOT check freshness (the edge does that against its own clock) or
    /// that the manifest's owner matches — those are policy checks.
    pub fn verify(&self) -> bool {
        let expected_owner = format!(
            "canopee://identity/{}",
            match libp2p::identity::PublicKey::try_decode_protobuf(&self.public_key) {
                Ok(key) => libp2p::PeerId::from(key.clone()).to_string(),
                Err(_) => return false,
            }
        );
        if expected_owner != self.identity.to_string() {
            return false;
        }
        let public_key = match libp2p::identity::PublicKey::try_decode_protobuf(&self.public_key) {
            Ok(key) => key,
            Err(_) => return false,
        };
        let bytes = Self::signing_bytes(&self.app_id, &self.identity, self.timestamp);
        public_key.verify(&bytes, &self.signature)
    }
}

/// The subdomain an app is served under: the first 32 hex chars of its
/// manifest id (128 bits — collision-proof, and fits the 63-char DNS label
/// limit that the full 64-char id would exceed).
pub fn app_subdomain(app_id: &str) -> &str {
    app_id.get(..32).unwrap_or(app_id)
}

/// The edge's verdict on a registration. `Ok` tells the publisher the edge has
/// pinned its subdomain to this connection.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ServeRegistrationResponse {
    Ok,
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use canopee_identity::Identity;

    #[tokio::test]
    async fn serve_registration_signs_and_verifies() {
        let identity = Identity::create("/tmp/canopee_serve_reg_a.key")
            .await
            .unwrap();
        let timestamp = 1_700_000_000;
        let registration = ServeRegistration::sign(&identity, "deadbeef", timestamp).unwrap();

        assert!(registration.verify());
        assert_eq!(registration.identity, *identity.id());
    }

    #[tokio::test]
    async fn serve_registration_rejects_tampering() {
        let identity = Identity::create("/tmp/canopee_serve_reg_b.key")
            .await
            .unwrap();
        let mut registration =
            ServeRegistration::sign(&identity, "deadbeef", 1_700_000_000).unwrap();

        registration.app_id = "cafebabe".to_string();
        assert!(!registration.verify());

        let other = Identity::create("/tmp/canopee_serve_reg_c.key")
            .await
            .unwrap();
        let mut forged = ServeRegistration::sign(&other, "deadbeef", 1_700_000_000).unwrap();
        forged.identity = registration.identity.clone();
        assert!(
            !forged.verify(),
            "an identity signing another's id must fail"
        );
    }

    #[test]
    fn app_subdomain_truncates_to_dns_label() {
        let id = "a".repeat(64);
        assert_eq!(app_subdomain(&id).len(), 32);
        assert_eq!(app_subdomain("short"), "short");
    }
}
