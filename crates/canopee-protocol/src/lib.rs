use canopee_identity::IdentityId;
use canopee_storage::{
    AppPointerRecord, ContactList, ExportBundle, HomeIndex, Object, ObjectId, ObjectInfo,
    ObjectType, Profile,
};
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RelayReservationInfo {
    pub relay_peer_id: String,
    pub renewal: bool,
    pub listen_addrs: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NodeCommand {
    Put {
        data: Vec<u8>,
    },
    PutObject {
        data: Vec<u8>,
        object_type: ObjectType,
    },
    Get {
        id: ObjectId,
    },
    List,
    Export {
        id: ObjectId,
    },
    Import {
        bundle: ExportBundle,
    },
    Status,
    Identity,
    Shutdown,
    Dial {
        addr: String,
    },
    ListenViaRelay {
        relay_addr: String,
    },
    Publish {
        topic: String,
        data: Vec<u8>,
    },
    /// Hijacks the connection: after `Subscribed` is sent, the node keeps
    /// pushing `PubSubMessage` frames on this same stream until the client
    /// disconnects. Not a request/response command like the others.
    Subscribe {
        topic: String,
    },
    Peers,
    RelayReservations,
    FindProviders {
        id: ObjectId,
    },
    FetchObject {
        peer_id: String,
        id: ObjectId,
    },
    Announce {
        id: ObjectId,
    },
    /// Signs (with the node's own identity) and publishes an `AppPointerRecord`
    /// mapping `name` to `manifest`, so peers who don't know `manifest`'s id
    /// yet can resolve it via `ResolveAppPointer { owner: <this node's
    /// identity>, name }`. Republishing under the same `name` overwrites the
    /// previous pointer.
    PublishAppPointer {
        name: String,
        manifest: ObjectId,
    },
    /// Looks up the latest `AppPointerRecord` published by `owner` under `name`.
    ResolveAppPointer {
        owner: IdentityId,
        name: String,
    },
    /// Signs and publishes a user record (`(owner, name)` → `ObjectId`) via
    /// the Runtime's cache-aware pointer layer.
    PublishPointer {
        name: String,
        target: ObjectId,
    },
    /// Resolves a user record the Runtime way: checks the local record cache
    /// first, then the DHT, verifying owner + signature.
    ResolvePointer {
        owner: IdentityId,
        name: String,
    },
    /// Stores a new `Profile` version and repoints `(owner, "profile")`.
    SaveProfile { profile: Profile },
    /// Loads the current `Profile` from the local shared store.
    LoadProfile,
    /// Stores a new `ContactList` version and repoints `(owner, "contacts")`.
    SaveContactList { list: ContactList },
    /// Loads the current `ContactList` from the local shared store.
    LoadContactList,
    /// Stores a new `HomeIndex` version and repoints `(owner, "home")`.
    SaveHomeIndex { index: HomeIndex },
    /// Loads the current `HomeIndex` from the local shared store.
    LoadHomeIndex,
    /// Flips one home entry's `shared` flag (the explicit "share this on the
    /// network" / "stop sharing" action) and republishes the index.
    SetHomeEntryShared { name: String, shared: bool },
    /// Shares a stored object under `name`: upserts a `shared: true` home
    /// entry and announces the object as a DHT provider.
    ShareObject { name: String, object: ObjectId, app: Option<String> },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NodeResponse {
    ObjectCreated {
        id: ObjectId,
    },
    Object {
        object: Object,
    },
    Objects {
        objects: Vec<ObjectInfo>,
    },
    Exported {
        bundle: ExportBundle,
    },
    Imported,
    Status {
        identity: String,
        objects: usize,
        peers: usize,
    },
    Error {
        message: String,
    },
    Identity {
        identity_id: IdentityId,
    },
    ShutdownAccepted,
    Dialed,
    ListeningViaRelay,
    Published,
    /// First frame sent for a `Subscribe` command; every following frame on
    /// the same connection is a `PubSub(PubSubMessage)` until disconnect.
    Subscribed,
    PubSub(PubSubMessage),
    Peers {
        peers: Vec<PeerInfo>,
    },
    RelayReservations {
        reservations: Vec<RelayReservationInfo>,
    },
    Providers {
        peer_ids: Vec<String>,
    },
    Announced,
    AppPointerPublished,
    AppPointer {
        record: Option<AppPointerRecord>,
    },
    PointerPublished,
    Pointer {
        record: Option<AppPointerRecord>,
    },
    ProfileSaved {
        id: ObjectId,
    },
    Profile {
        profile: Option<Profile>,
    },
    ContactListSaved {
        id: ObjectId,
    },
    ContactList {
        list: Option<ContactList>,
    },
    HomeIndexSaved {
        id: ObjectId,
    },
    HomeIndex {
        index: Option<HomeIndex>,
    },
}
