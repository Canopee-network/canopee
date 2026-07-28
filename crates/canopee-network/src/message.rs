use canopee_storage::{ExportBundle, ObjectId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ObjectRequest {
    GetObject(ObjectId),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ObjectResponse {
    Object(ExportBundle),
    NotFound,
}

#[derive(Debug, Clone)]
pub struct PubSubMessage {
    pub topic: String,
    pub source: Option<libp2p::PeerId>,
    pub data: Vec<u8>,
}
