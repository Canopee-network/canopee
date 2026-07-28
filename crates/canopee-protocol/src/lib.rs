use canopee_identity::IdentityId;
use canopee_storage::{ExportBundle, Object, ObjectId, ObjectInfo};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PubSubMessage {
    pub topic: String,
    pub source: Option<String>,
    pub data: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PeerInfo {
    pub peer_id: String,
    pub identity: Option<IdentityId>,
    pub addresses: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NodeCommand {
    Put { data: Vec<u8> },
    Get { id: ObjectId },
    List,
    Export { id: ObjectId },
    Import { bundle: ExportBundle },
    Status,
    Identity,
    Shutdown,
    Dial { addr: String },
    ListenViaRelay { relay_addr: String },
    Publish { topic: String, data: Vec<u8> },
    /// Hijacks the connection: after `Subscribed` is sent, the node keeps
    /// pushing `PubSubMessage` frames on this same stream until the client
    /// disconnects. Not a request/response command like the others.
    Subscribe { topic: String },
    Peers,
    FindProviders { id: ObjectId },
    FetchObject { peer_id: String, id: ObjectId },
    Announce { id: ObjectId },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NodeResponse {
    ObjectCreated { id: ObjectId },
    Object { object: Object },
    Objects { objects: Vec<ObjectInfo> },
    Exported { bundle: ExportBundle },
    Imported,
    Status {
        identity: String,
        objects: usize,
        peers: usize,
    },
    Error { message: String },
    Identity { identity_id: IdentityId },
    ShutdownAccepted,
    Dialed,
    ListeningViaRelay,
    Published,
    /// First frame sent for a `Subscribe` command; every following frame on
    /// the same connection is a `PubSub(PubSubMessage)` until disconnect.
    Subscribed,
    PubSub(PubSubMessage),
    Peers { peers: Vec<PeerInfo> },
    Providers { peer_ids: Vec<String> },
    Announced,
}
