use crate::object_id::ObjectId;
use canopee_identity::{Identity, IdentityId};
use libp2p::identity::PublicKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

/// A signed pointer from a stable `(owner, name)` pair to the latest
/// `AppManifest` object id. Unlike an `Object`, this isn't content-addressed
/// — it's stored in the DHT under a key derived from `owner` + `name`, so
/// republishing an app under the same name lets fetchers resolve the newest
/// manifest without needing a new id out of band.
///
/// Signed (not just stored as plain DHT data) so a fetcher that receives one
/// from an arbitrary peer can verify it actually came from the claimed
/// owner. `published_at` is included in the signed bytes (so a captured
/// record can't be replayed with a different timestamp) but nothing
/// currently compares it against a previously-seen value — callers don't
/// yet reject an old pointer served back in place of a newer one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppPointerRecord {
    pub name: String,
    pub owner: IdentityId,
    pub manifest: ObjectId,
    pub published_at: u64,
    public_key: Vec<u8>,
    signature: Vec<u8>,
}

impl AppPointerRecord {
    pub fn sign(identity: &Identity, name: &str, manifest: ObjectId) -> anyhow::Result<Self> {
        let owner = identity.id().clone();
        let published_at = OffsetDateTime::now_utc().unix_timestamp() as u64;
        let signature =
            identity.sign(&Self::signing_bytes(name, &owner, &manifest, published_at))?;

        Ok(Self {
            name: name.to_string(),
            owner,
            manifest,
            published_at,
            public_key: identity.public_key_bytes(),
            signature,
        })
    }

    /// The DHT key both publisher and fetcher derive independently from
    /// `owner` + `name` — no need to exchange it out of band.
    pub fn key(owner: &IdentityId, name: &str) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(b"canopee-app-pointer:");
        hasher.update(owner.to_string().as_bytes());
        hasher.update(b":");
        hasher.update(name.as_bytes());
        hasher.finalize().to_vec()
    }

    fn signing_bytes(
        name: &str,
        owner: &IdentityId,
        manifest: &ObjectId,
        published_at: u64,
    ) -> Vec<u8> {
        format!("{name}:{owner}:{manifest}:{published_at}").into_bytes()
    }

    /// Verifies the signature was produced by the record's claimed owner,
    /// and that the owner's public key actually corresponds to their
    /// `IdentityId` (preventing a peer from attaching someone else's name
    /// to their own signature).
    pub fn verify(&self) -> bool {
        let expected_owner = format!(
            "canopee://identity/{}",
            match PublicKey::try_decode_protobuf(&self.public_key) {
                Ok(key) => libp2p::PeerId::from(key.clone()).to_string(),
                Err(_) => return false,
            }
        );
        if expected_owner != self.owner.to_string() {
            return false;
        }
        let public_key = match PublicKey::try_decode_protobuf(&self.public_key) {
            Ok(key) => key,
            Err(_) => return false,
        };
        let bytes = Self::signing_bytes(&self.name, &self.owner, &self.manifest, self.published_at);
        public_key.verify(&bytes, &self.signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn signed_record_verifies_and_key_is_deterministic() {
        let identity = Identity::create("/tmp/canopee_pointer_test_a.key")
            .await
            .unwrap();
        let manifest = ObjectId::new("manifest-1");
        let record =
            AppPointerRecord::sign(&identity, "alice-portfolio", manifest.clone()).unwrap();

        assert!(record.verify());
        assert_eq!(record.manifest, manifest);
        assert_eq!(
            AppPointerRecord::key(&record.owner, "alice-portfolio"),
            AppPointerRecord::key(identity.id(), "alice-portfolio")
        );
    }

    #[tokio::test]
    async fn tampered_manifest_fails_verification() {
        let identity = Identity::create("/tmp/canopee_pointer_test_b.key")
            .await
            .unwrap();
        let mut record =
            AppPointerRecord::sign(&identity, "alice-portfolio", ObjectId::new("manifest-1"))
                .unwrap();

        record.manifest = ObjectId::new("manifest-evil");

        assert!(!record.verify());
    }

    #[tokio::test]
    async fn spoofed_owner_fails_verification() {
        let identity = Identity::create("/tmp/canopee_pointer_test_c.key")
            .await
            .unwrap();
        let mut record =
            AppPointerRecord::sign(&identity, "alice-portfolio", ObjectId::new("manifest-1"))
                .unwrap();

        record.owner = IdentityId::new("canopee://identity/not-actually-alice");

        assert!(!record.verify());
    }
}
