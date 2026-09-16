use canopee_protocol::PairingPayload;
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
