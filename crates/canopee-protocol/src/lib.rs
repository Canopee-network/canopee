use canopee_identity::IdentityId;
use canopee_storage::{
    AppPointerRecord, Capability, CapabilityId, CapabilityIndex, ContactList, ExportBundle,
    HomeIndex, Object, ObjectId, ObjectInfo, ObjectType, Permission, Profile, Resource,
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
    /// The peer's claimed username, resolved opportunistically from its
    /// signed `(owner, "username")` record (see `Peer.username`).
    pub username: Option<String>,
    /// The peer's profile display name, resolved from its signed
    /// `(owner, "profile")` record.
    pub display_name: Option<String>,
    pub addresses: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RelayReservationInfo {
    pub relay_peer_id: String,
    pub renewal: bool,
    pub listen_addrs: Vec<String>,
}

/// One device currently carrying an identity, as presented to the CLI/UI.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
}

/// Everything the *new* device publishes out of band (QR / printed) to start
/// a pairing: its own device info, a dialable LAN address, the 12-char pairing
/// code shown for the source device's user to type/scan, and a one-time
/// `session_id` scoping the exchange.
///
/// The pairing code is deliberately the *full* 12-char value: the QR/printed
/// form is the "out of band" channel that vouches for it. Over the peer-to-peer
/// wire only `device_id` + `session_id` travel (see `CanopeePairingRequest`),
/// so the code itself is never transmitted across the network.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PairingQrData {
    pub version: u8,
    pub device_id: String,
    pub device_name: String,
    /// Dialable multiaddr for this (new) device, e.g.
    /// `/ip4/192.168.1.5/tcp/34567/p2p/12D3KooW…`.
    pub lan_addr: String,
    pub code: String,
    pub session_id: String,
}

/// One signed record (object + its `(owner, name)` pointer) transferred during
/// pairing, so the new device can store and verify it offline.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PairingRecord {
    pub name: String,
    pub object: Object,
    pub pointer: AppPointerRecord,
}

/// The cleartext contents of a pairing payload: the identity signing key and
/// a set of signed user records (device list, profile, contacts). Encrypted as
/// a whole (bincode) under the session key before hitting the wire.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PairingData {
    pub version: u8,
    /// Raw Ed25519 identity keypair protobuf bytes (see `Identity::export_bytes`).
    pub identity_key: Vec<u8>,
    pub records: Vec<PairingRecord>,
}

/// The AEAD-encrypted pairing payload: `nonce || ciphertext || tag`, produced
/// from `PairingData` by `Runtime::complete_pairing`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PairingPayload {
    pub encrypted: Vec<u8>,
}

/// The outcome of a sync pass: which user records were refreshed from the
/// network (a newer signed pointer was found on the DHT and its object
/// imported into local storage).
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncResult {
    pub profile_updated: bool,
    pub contacts_updated: bool,
    pub devices_updated: bool,
}

impl SyncResult {
    /// True when at least one record was refreshed.
    pub fn any_updated(&self) -> bool {
        self.profile_updated || self.contacts_updated || self.devices_updated
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NodeCommand {
    Put {
        data: Vec<u8>,
        name: Option<String>,
    },
    PutObject {
        data: Vec<u8>,
        object_type: ObjectType,
        name: Option<String>,
    },
    Get {
        id: ObjectId,
    },
    List,
    /// Gives a stored object a human-readable name (stored alongside the
    /// object, never inside the signed bundle). Local convenience so listings
    /// and resolution can stay id-free.
    SetName {
        id: ObjectId,
        name: String,
    },
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
    /// Claims a globally unique username for this node's identity (publishes
    /// the signed `(owner, "username")` record + the DHT registry entry).
    ClaimUsername { username: String },
    /// Reverse-resolves a friendly username to its canonical owner via the
    /// DHT registry (spoof-verified against the owner's signed record).
    ResolveUsername { username: String },
    /// Returns the identity key as a transferable, encrypted envelope (see
    /// `Identity::export_encrypted`) — the "move my identity to another
    /// device" action. `passphrase` protects the exported bytes in transit.
    ExportIdentity {
        passphrase: String,
    },
    /// Imports an identity key previously exported via `ExportIdentity` and
    /// persists it to the node's identity file. `overwrite` must be set to
    /// replace an existing identity on disk (`false` refuses if one exists).
    /// Only takes effect after the node restarts.
    ImportIdentity {
        bytes: Vec<u8>,
        passphrase: String,
        overwrite: bool,
    },
    /// Returns this identity's currently claimed username (its `(owner,
    /// "username")` record), if any.
    ShowUsername,
    /// Returns this device's network `PeerId` (from its per-device key) and
    /// human-friendly name.
    Device,
    /// Lists the devices currently carrying this node's identity, via the
    /// `(owner, "devices")` record.
    DeviceList,
    /// Resolves which device peer id to dial to reach `owner`, via its
    /// `(owner, "devices")` list. `None` when the owner has no registered,
    /// well-formed device.
    ResolveOwnerDevice { owner: IdentityId },
    /// Records another device against this node's `(owner, "devices")` list
    /// and republishes it. Used by pairing/bonding to admit a new machine.
    AddDevice {
        device_id: String,
        device_name: String,
    },
    /// Removes a device from this node's `(owner, "devices")` list and
    /// republishes it.
    RemoveDevice { device_id: String },
    /// Starts a device-pairing session on THIS device (the new device): mints
    /// a fresh 12-char code + session id and returns the `PairingQrData` to
    /// show/print out of band. The node keeps the code in memory so it can
    /// decrypt the payload the source device sends back.
    InitiatePairing,
    /// Completes a pairing initiated on another device: verifies the typed
    /// code against the QR data, encrypts this device's identity + user
    /// records, dials the other device on the LAN and delivers the payload.
    /// Returns a human-readable status message on success.
    CompletePairing {
        qr: PairingQrData,
        code: String,
    },
    /// Refreshes this node's user records (profile, contacts, devices) from
    /// the network, optionally dialing `peer_id` first to ensure the peer is
    /// reachable. Last-writer-wins: a newer signed pointer on the DHT
    /// replaces the local cache and its object is imported.
    SyncFromPeer { peer_id: String },
    /// Refreshes this node's user records from every device in its
    /// `(owner, "devices")` list. Equivalent to `SyncFromPeer` for each
    /// registered device, but the record refresh itself is identity-scoped
    /// (all devices share the same DHT keys), so it runs once.
    SyncDeviceList,
    /// Issues a signed `Capability` on behalf of this node's identity,
    /// granting `subject` `permissions` over `resource`, optionally expiring
    /// at `expires_at` (unix seconds). Stores it and records it in the
    /// issuer's `(owner, "capabilities")` index. The issued capability is
    /// returned so the caller can pass it to the subject out of band.
    GrantCapability {
        subject: IdentityId,
        resource: Resource,
        permissions: Vec<Permission>,
        expires_at: Option<u64>,
    },
    /// Lists the capability grants this node's identity has issued, via the
    /// `(owner, "capabilities")` record (revocation state included).
    ListCapabilities,
    /// Marks one previously issued capability as revoked in the index and
    /// republishes it. `null`/empty id optional for symmetry; the id is the
    /// content-derived `Capability.id`.
    RevokeCapability {
        id: CapabilityId,
    },
    /// Verifies a presented capability end to end: signature + id + issuer
    /// derivation, time window, and (when the issuer's index is reachable)
    /// revocation state. The answer apps run before serving a resource.
    CheckCapability {
        capability: Capability,
    },
    /// The issuer-side authorization question: does `subject` currently hold
    /// an unrevoked, unexpired grant from this node's identity of `permission`
    /// on `resource`? The check run before serving a resource to a peer.
    CheckAccess {
        subject: IdentityId,
        permission: Permission,
        resource: Resource,
    },
    /// Starts a publish/serve session for the app whose manifest is
    /// `app_id`: signs a registration, dials the Canopee edge at `edge_addr`
    /// (`/p2p/...` multiaddr) and pins `<app-hash>.<base>.domain` to this
    /// connection where the edge forwards HTTP traffic. Foreground: the CLI
    /// keeps the node running until `StopServeSession` (Ctrl+C) ends it.
    StartServeSession {
        edge_addr: String,
        app_id: ObjectId,
    },
    /// Deregisters the current serve session with the edge and stops its
    /// heartbeat. No-op when no session is running.
    StopServeSession,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NodeResponse {
    ObjectCreated {
        id: ObjectId,
    },
    NameSet,
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
    UsernameClaimed,
    Username {
        username: Option<String>,
    },
    UsernameOwner {
        owner: Option<IdentityId>,
    },
    /// The encrypted envelope produced by `ExportIdentity` — raw bytes, ready
    /// to be written to a file or handed to another device.
    IdentityExported {
        bytes: Vec<u8>,
    },
    /// Confirms an `ImportIdentity` succeeded and reports the identity the
    /// node will adopt on restart.
    IdentityImported {
        identity_id: IdentityId,
    },
    /// The response to `Device`: this machine's network identity.
    Device {
        peer_id: String,
        device_name: String,
    },
    /// The response to `DeviceList`.
    DeviceList {
        devices: Vec<DeviceInfo>,
    },
    /// The response to `ResolveOwnerDevice`: the peer id to dial, if the
    /// owner has any registered device.
    OwnerDevice {
        peer_id: Option<String>,
    },
    DeviceAdded,
    DeviceRemoved,
    /// The response to `InitiatePairing`: the QR data to display/print.
    PairingQr {
        qr: PairingQrData,
    },
    /// Confirms a `CompletePairing` round-trip finished (payload accepted by
    /// the other device, or a transport error surfaced in `Error`).
    PairingComplete {
        message: String,
    },
    /// The response to `SyncFromPeer` / `SyncDeviceList`: which records were
    /// refreshed from the network.
    SyncComplete { result: SyncResult },
    /// The response to `GrantCapability`: the newly issued grant.
    CapabilityGranted {
        capability: Capability,
    },
    /// The response to `ListCapabilities`: the issuer's full index (revocation
    /// state included), or `None` when nothing has been issued yet.
    Capabilities {
        index: Option<CapabilityIndex>,
    },
    /// Confirms `RevokeCapability` marked the grant revoked and republished
    /// the index.
    CapabilityRevoked,
    /// The response to `CheckCapability`: `valid` plus a human-readable
    /// `reason` explaining an invalid verdict.
    CapabilityCheck {
        valid: bool,
        reason: String,
    },
    /// The response to `CheckAccess`: whether the subject currently holds the
    /// requested grant from this node's identity.
    AccessAllowed {
        allowed: bool,
    },
    /// The response to `StartServeSession`: the session is live and the edge
    /// has pinned the app to this connection. `root_url` is the public root
    /// (`https://<app-hash>.<base>/`) the app is served at.
    ServeSessionStarted {
        app_id: String,
        root_url: String,
    },
    /// Confirms `StopServeSession` ended the previous session (or that none
    /// was running).
    ServeSessionStopped,
}
