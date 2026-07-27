use canopee_identity::IdentityId;
use canopee_storage::{ExportBundle, ObjectId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum NetworkMessage {
    Hello(Hello),
    Welcome(Welcome),
    Ping,
    Pong,
    FindObject(ObjectId),
    HaveObject(ObjectId),
    GetObject(ObjectId),
    Object(ExportBundle),
    AnnounceObject(ObjectId),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    pub identity: IdentityId,
    // pub public_key: Vec<u8>,
    // pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Welcome {
    pub protocol: u32,
    pub accepted: bool,
    pub identity: IdentityId,
}
