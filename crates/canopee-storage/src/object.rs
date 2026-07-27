use crate::export_bundle::ExportBundle;
use crate::object_id::ObjectId;
use canopee_identity::{Identity, IdentityId};
use libp2p::identity::PublicKey;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/*          |--- ObjectMetadata
 * Object ------ ObjectId
 *          \--- ObjectPayload
 *           \-- Signature
 */

pub type Signature = Vec<u8>;
pub type PublicKeyBytes = Vec<u8>;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ObjectMetadata {
    pub created_at: u64,
    pub size: u64,
    pub content_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ObjectPayload {
    pub owner: IdentityId,
    pub metadata: ObjectMetadata,
    pub data: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Object {
    pub id: ObjectId,
    pub payload: ObjectPayload,
    pub public_key: PublicKeyBytes,
    pub signature: Signature,
}

impl Object {
    pub fn new(identity: &Identity, bytes: Vec<u8>) -> Self {
        let metadata = ObjectMetadata {
            created_at: OffsetDateTime::now_utc().unix_timestamp() as u64,
            size: bytes.len() as u64,
            content_type: None,
        };
        let payload = ObjectPayload {
            owner: identity.id().clone(),
            metadata,
            data: bytes,
        };
        let id = ObjectId::from_payload(&payload);
        let encoded = bincode::serialize(&payload).unwrap();
        let signature = identity.sign(&encoded).unwrap();

        Self {
            id,
            payload,
            public_key: identity.public_key_bytes(),
            signature,
        }
    }
    pub fn verify_id(&self) -> bool {
        let calculated = ObjectId::from_payload(&self.payload);
        calculated == self.id
    }
}

pub trait Verify {
    fn verify(&self) -> bool;
}

impl Verify for Object {
    fn verify(&self) -> bool {
        if !self.verify_id() {
            return false;
        }
        let payload = bincode::serialize(&self.payload).unwrap();
        let public_key = PublicKey::try_decode_protobuf(&self.public_key);
        match public_key {
            Ok(key) => key.verify(&payload, &self.signature),

            Err(_) => false,
        }
    }
}

pub trait Export {
    fn export(&self) -> anyhow::Result<ExportBundle>;
}

impl Export for Object {
    fn export(&self) -> anyhow::Result<ExportBundle> {
        if !self.verify() {
            anyhow::bail!("Cannot export invalid object");
        }
        Ok(ExportBundle {
            version: 1,
            object: self.clone(),
        })
    }
}
