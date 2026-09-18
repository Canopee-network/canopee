//! Signed capabilities: "who is allowed to do what", verifiable by anyone
//! without a centralized authorization server (technical vision §5.5).
//!
//! A `Capability` is a signed document from an `issuer` granting a `subject`
//! a set of `Permission`s over a `Resource`, optionally expiring. Because it
//! is self-contained (it carries the issuer's public key and a signature over
//! its canonical bytes), any peer that receives a capability — directly, via a
//! pointer, or inside an object — can verify it cryptographically against the
//! issuer, exactly like an `AppPointerRecord`.
//!
//! Grants an issuer has made are tracked in a signed `CapabilityIndex`
//! published under the `(owner, "capabilities")` record (the same pattern as
//! `DeviceList`), so issuers can list and revoke their grants. Revocation is
//! "publish a new index with the entry marked revoked" — the same immutable
//! object + mutable record model used everywhere else in the user store.

use crate::{Object, ObjectId, ObjectType};
use canopee_identity::{Identity, IdentityId};
use libp2p::identity::PublicKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The reserved record name under which a user publishes the index of
/// capability grants they've issued: `(owner, "capabilities")` → a signed
/// `CapabilityIndex` object.
pub const RECORD_CAPABILITIES: &str = "capabilities";

/// What a subject is allowed to do on a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    Read,
    Write,
    Publish,
}

impl Permission {
    /// Canonical wire/string name, used in signing bytes and CLI output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Permission::Read => "read",
            Permission::Write => "write",
            Permission::Publish => "publish",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "read" => Some(Permission::Read),
            "write" => Some(Permission::Write),
            "publish" => Some(Permission::Publish),
            _ => None,
        }
    }
}

/// What a capability grants access to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Resource {
    /// A single content-addressed object.
    Object(ObjectId),
    /// A named entry in an owner's home index (`(owner, name)`).
    SharedName { owner: IdentityId, name: String },
    /// A pubsub channel / gossipsub topic.
    Channel(String),
}

impl Resource {
    /// Canonical form used in signing bytes, matching `Permission::as_str`.
    pub fn canonical(&self) -> String {
        match self {
            Resource::Object(id) => format!("object:{id}"),
            Resource::SharedName { owner, name } => format!("shared:{owner}:{name}"),
            Resource::Channel(topic) => format!("channel:{topic}"),
        }
    }

    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let (kind, rest) = s
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("resource must be object:<id> | channel:<topic> | shared:<owner>:<name>"))?;
        match kind {
            "object" => Ok(Resource::Object(ObjectId::new(rest))),
            "channel" => Ok(Resource::Channel(rest.to_string())),
            "shared" => {
                let (owner, name) = rest
                    .split_once(':')
                    .ok_or_else(|| anyhow::anyhow!("shared resource must be shared:<owner>:<name>"))?;
                Ok(Resource::SharedName {
                    owner: IdentityId::new(owner),
                    name: name.to_string(),
                })
            }
            other => anyhow::bail!("unknown resource kind \"{other}\""),
        }
    }
}

/// Stable id for a capability, used for listing and targeted revocation.
/// Derived from the capability's content, so identical grants collapse to one
/// id and verification can recompute it without trusting the transmitted value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CapabilityId(pub String);

impl std::fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A signed grant: `issuer` lets `subject` exercise `permissions` on
/// `resource` until `expires_at` (or indefinitely). The signature is over the
/// canonical serialization of every field except `public_key`/`signature`, and
/// `id` is a content-derived digest of those same fields — so a captured or
/// attacker-modified capability cannot re-verify.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    pub id: CapabilityId,
    pub issuer: IdentityId,
    pub subject: IdentityId,
    pub resource: Resource,
    pub permissions: Vec<Permission>,
    /// Unix timestamp (seconds). 0 for "valid from the epoch" (implicitly: any time).
    pub issued_at: u64,
    pub expires_at: Option<u64>,
    public_key: Vec<u8>,
    signature: Vec<u8>,
}

impl Capability {
    /// Signs and mints a new capability on behalf of `issuer`. `expires_at`
    /// is a unix timestamp; `None` grants without expiry.
    pub fn issue(
        issuer: &Identity,
        subject: IdentityId,
        resource: Resource,
        permissions: Vec<Permission>,
        expires_at: Option<u64>,
    ) -> anyhow::Result<Self> {
        let issued_at = now_ts();
        let fields = Self::signing_fields(
            &subject,
            &resource,
            &permissions,
            issued_at,
            expires_at,
        );
        let id = digest_hex(fields.as_bytes());
        let signing_bytes = Self::signing_bytes(&id, &fields);
        let signature = issuer.sign(&signing_bytes)?;
        Ok(Self {
            id: CapabilityId(id),
            issuer: issuer.id().clone(),
            subject,
            resource,
            permissions,
            issued_at,
            expires_at,
            public_key: issuer.public_key_bytes(),
            signature,
        })
    }

    /// The fields that participate in the id digest and signature, in canonical
    /// form. Permissions are sorted so element order can't manufacture two
    /// different-signing-byte capabilities for the same grant.
    fn signing_fields(
        subject: &IdentityId,
        resource: &Resource,
        permissions: &[Permission],
        issued_at: u64,
        expires_at: Option<u64>,
    ) -> String {
        let mut perms = permissions.iter().map(|p| p.as_str()).collect::<Vec<_>>();
        perms.sort_unstable();
        format!(
            "{subject}:{}:{}:{issued_at}:{}",
            resource.canonical(),
            perms.join(","),
            expires_at.map(|t| t.to_string()).unwrap_or_default(),
        )
    }

    fn signing_bytes(id: &str, fields: &str) -> Vec<u8> {
        format!("canopee/capability/v1:{id}:{fields}").into_bytes()
    }

    /// Recomputes the fields this capability was signed over.
    fn fields(&self) -> String {
        Self::signing_fields(
            &self.subject,
            &self.resource,
            &self.permissions,
            self.issued_at,
            self.expires_at,
        )
    }

    /// True when the signature is valid, the id matches the content, and the
    /// capsule's public key actually derives the claimed issuer identity. Pure
    /// cryptography — time/expiry and revocation are enforced separately by
    /// callers so a captured-but-valid signature stays verifiable forever.
    pub fn verify(&self) -> bool {
        let expected_issuer = format!(
            "canopee://identity/{}",
            match PublicKey::try_decode_protobuf(&self.public_key) {
                Ok(key) => libp2p::PeerId::from(key.clone()).to_string(),
                Err(_) => return false,
            }
        );
        if expected_issuer != self.issuer.to_string() {
            return false;
        }
        let fields = self.fields();
        if self.id.0 != digest_hex(fields.as_bytes()) {
            return false;
        }
        let bytes = Self::signing_bytes(&self.id.0, &fields);
        match PublicKey::try_decode_protobuf(&self.public_key) {
            Ok(key) => key.verify(&bytes, &self.signature),
            Err(_) => false,
        }
    }

    /// True at unix time `now` if the capability has taken effect (issued) and
    /// hasn't expired. Revocation is tracked separately in the issuer's
    /// `CapabilityIndex`.
    pub fn is_active_at(&self, now: u64) -> bool {
        now >= self.issued_at
            && self.expires_at.map(|t| now <= t).unwrap_or(true)
    }

    /// True when this capability grants `permission` on `resource` to
    /// `subject`, is currently valid (signature + id + issuer), and active at
    /// `now`. This is the one-call "should I let this peer through?" check an
    /// app runs before serving a resource.
    pub fn permits(&self, subject: &IdentityId, permission: Permission, resource: &Resource, now: u64) -> bool {
        subject == &self.subject
            && &self.resource == resource
            && self.permissions.contains(&permission)
            && self.verify()
            && self.is_active_at(now)
    }

    pub fn export(&self) -> anyhow::Result<ExportCapability> {
        Ok(ExportCapability {
            version: 1,
            capability: self.clone(),
        })
    }

    /// Wraps this signed capability in a content-addressed `Object` (type
    /// `Capability`), so it can be stored, exported, and passed to a peer like
    /// any other object — its signature lives inside, so it survives transport
    /// intact.
    pub fn to_object(&self, identity: &Identity) -> anyhow::Result<Object> {
        Ok(Object::new(
            identity,
            bincode::serialize(self)?,
            ObjectType::Capability,
        ))
    }
}

/// A portable, self-contained capability bundle (mirrors `ExportBundle` for
/// objects): escape hatch for handing a signed grant to the subject out of
/// band — over a file, a QR, a chat message — who then verifies it locally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportCapability {
    pub version: u8,
    pub capability: Capability,
}

impl ExportCapability {
    pub fn verify(&self) -> bool {
        self.version == 1 && self.capability.verify()
    }
}

/// One grant in an issuer's index, with its revocation state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityEntry {
    pub capability: Capability,
    pub revoked: bool,
}

/// A signed snapshot of the capability grants an owner has issued. Published
/// under `(owner, "capabilities")`; issuing appends an entry, revoking flips
/// its flag, and editing republishes the index — the immutable object +
/// mutable record model used by `DeviceList` and `HomeIndex`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityIndex {
    pub entries: Vec<CapabilityEntry>,
    /// Incremented per edit; purely informational (the record timestamp is the
    /// authoritative ordering).
    pub version: u64,
}

impl CapabilityIndex {
    pub fn to_object(&self, identity: &Identity) -> anyhow::Result<Object> {
        Ok(Object::new(
            identity,
            bincode::serialize(self)?,
            ObjectType::CapabilityIndex,
        ))
    }

    pub fn by_id(&self, id: &CapabilityId) -> Option<&CapabilityEntry> {
        self.entries.iter().find(|e| e.capability.id == *id)
    }

    /// The (first) unrevoked grant matching `subject` + `resource` +
    /// `permission`, if a currently-active one exists at `now`.
    pub fn find_active(
        &self,
        subject: &IdentityId,
        resource: &Resource,
        permission: Permission,
        now: u64,
    ) -> Option<&Capability> {
        self.entries.iter().find_map(|entry| {
            let cap = &entry.capability;
            (!entry.revoked
                && cap.permits(subject, permission, resource, now))
            .then_some(cap)
        })
    }

    /// `true` if `subject` currently holds an unrevoked, unexpired grant of
    /// `permission` on `resource`. The issuer-side authorization question:
    /// "am I allowed to let them do this?" (vision §5.5).
    pub fn grants(
        &self,
        subject: &IdentityId,
        resource: &Resource,
        permission: Permission,
        now: u64,
    ) -> bool {
        self.find_active(subject, resource, permission, now).is_some()
    }
}

fn now_ts() -> u64 {
    time::OffsetDateTime::now_utc().unix_timestamp().max(0) as u64
}

fn digest_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn identity() -> Identity {
        let dir = std::env::temp_dir().join(format!(
            "canopee_storage_capability_test_{}_{}",
            std::process::id(),
            next_identity_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("identity.key");
        canopee_identity::Identity::create(path.to_str().unwrap())
            .await
            .unwrap()
    }

    fn next_identity_id() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    mod defaults {
        use super::*;
        pub fn resource() -> Resource {
            Resource::Object(ObjectId::new("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"))
        }
    }

    #[tokio::test]
    async fn issue_verify_and_permits() {
        let alice = identity().await;
        let bob = IdentityId::new("canopee://identity/bob");

        let cap = Capability::issue(
            &alice,
            bob.clone(),
            defaults::resource(),
            vec![Permission::Read, Permission::Write],
            None,
        )
        .unwrap();

        assert!(cap.verify(), "freshly issued capability must verify");
        let now = now_ts();
        assert!(cap.is_active_at(now));
        assert!(
            cap.permits(&bob, Permission::Read, &defaults::resource(), now),
            "bob may read the object"
        );
        assert!(
            !cap.permits(&bob, Permission::Publish, &defaults::resource(), now),
            "bob was not granted publish"
        );
        assert!(
            !cap.permits(
                &IdentityId::new("canopee://identity/mallory"),
                Permission::Read,
                &defaults::resource(),
                now
            ),
            "only the subject is permitted"
        );
        assert!(
            !cap.permits(
                &bob,
                Permission::Read,
                &Resource::Channel("other".into()),
                now
            ),
            "a different resource does not match"
        );
    }

    #[tokio::test]
    async fn tampering_breaks_verification() {
        let alice = identity().await;
        let bob = IdentityId::new("canopee://identity/bob");
        let resource = defaults::resource();
        let mut cap = Capability::issue(
            &alice,
            bob.clone(),
            resource.clone(),
            vec![Permission::Read],
            None,
        )
        .unwrap();

        // Resign under a different identity: issuer no longer derives the key.
        let mallory = identity().await;
        cap.public_key = mallory.public_key_bytes();
        assert!(!cap.verify(), "spoofed issuer must fail");

        // Tamper with the resource: signature + content id both break.
        let cap = Capability::issue(
            &alice,
            bob.clone(),
            resource.clone(),
            vec![Permission::Read],
            None,
        )
        .unwrap();
        let mut cap = cap;
        cap.resource = Resource::Channel("another-channel".into());
        assert!(!cap.verify(), "tampered resource must fail");

        // Tampered permissions: signature breaks.
        let mut cap = Capability::issue(
            &alice,
            bob.clone(),
            resource.clone(),
            vec![Permission::Read],
            None,
        )
        .unwrap();
        cap.permissions.push(Permission::Write);
        assert!(!cap.verify());
    }

    #[tokio::test]
    async fn expiry_window_is_enforced() {
        let alice = identity().await;
        let bob = IdentityId::new("canopee://identity/bob");

        // Expired one hour ago.
        let past = now_ts() - 3600;
        let cap = Capability::issue(
            &alice,
            bob.clone(),
            defaults::resource(),
            vec![Permission::Read],
            Some(past),
        )
        .unwrap();
        assert!(cap.verify(), "signature is still valid after expiry");
        assert!(
            !cap.permits(&bob, Permission::Read, &defaults::resource(), now_ts()),
            "an expired capability permits nothing"
        );
    }

    #[tokio::test]
    async fn identical_grants_collapse_to_one_id() {
        let alice = identity().await;
        let a = Capability::issue(
            &alice,
            IdentityId::new("canopee://identity/bob"),
            defaults::resource(),
            vec![Permission::Read],
            None,
        )
        .unwrap();
        // Force the same issued_at so the whole payload matches.
        let mut b = Capability::issue(
            &alice,
            IdentityId::new("canopee://identity/bob"),
            defaults::resource(),
            vec![Permission::Read],
            None,
        )
        .unwrap();
        b.issued_at = a.issued_at;
        assert_eq!(a.id, b.id, "identical grants share a content-derived id");
    }

    #[tokio::test]
    async fn index_tracks_revocation() {
        let alice = identity().await;
        let bob = IdentityId::new("canopee://identity/bob");
        let resource = defaults::resource();
        let cap = Capability::issue(
            &alice,
            bob.clone(),
            resource.clone(),
            vec![Permission::Read],
            None,
        )
        .unwrap();

        let mut index = CapabilityIndex {
            entries: vec![CapabilityEntry {
                capability: cap.clone(),
                revoked: false,
            }],
            version: 1,
        };
        let now = now_ts();
        assert!(index.grants(&bob, &resource, Permission::Read, now));

        index.entries[0].revoked = true;
        assert!(!index.grants(&bob, &resource, Permission::Read, now));
        assert!(index.by_id(&cap.id).is_some());
        assert!(index.by_id(&CapabilityId("nope".into())).is_none());
    }
}