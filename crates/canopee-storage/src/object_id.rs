use crate::object::ObjectPayload;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObjectId(pub String);

impl ObjectId {
    pub fn new(id: &str) -> Self {
        ObjectId(id.to_string())
    }

    pub fn from_payload(payload: &ObjectPayload) -> Self {
        let bytes = bincode::serialize(payload).expect("failed to serialize payload");
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash = hasher.finalize();

        Self(hex::encode(hash))
    }
    pub fn from_data(data: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash = hasher.finalize();

        Self(hex::encode(hash))
    }
}

impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
