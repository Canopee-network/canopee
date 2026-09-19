use crate::node_client::NodeClient;
use crate::subscription::Subscription;
use canopee_identity::IdentityId;
use canopee_protocol::{
    DeviceInfo, NodeCommand, NodeResponse, PairingQrData, PeerInfo, RelayReservationInfo,
    SyncResult,
};
use canopee_storage::{
    AppPointerRecord, Capability, CapabilityId, CapabilityIndex, ContactList, ExportBundle,
    HomeIndex, Object, ObjectId, ObjectInfo, ObjectType, Permission, Profile, Resource,
};
/// Entry point for apps that want to use a Canopee node's identity, storage,
/// and network capabilities. Talks to the locally running node over its Unix
/// socket; the node itself owns the identity keys, object storage, and the
/// libp2p swarm.
pub struct CanopeeClient {
    node: NodeClient,
}

impl CanopeeClient {
    /// Connects to the local Canopee node. Returns an error if the node
    /// isn't running (call `canopee start` / `canopee-node` first).
    pub async fn connect() -> anyhow::Result<Self> {
        let node = NodeClient::new().await?;
        if !node.is_running().await {
            anyhow::bail!("Canopee node is not running");
        }
        Ok(Self { node })
    }

    async fn request(&self, command: NodeCommand) -> anyhow::Result<NodeResponse> {
        self.node.request(command).await
    }

    fn unexpected(response: NodeResponse) -> anyhow::Error {
        match response {
            NodeResponse::Error { message } => anyhow::anyhow!(message),
            other => anyhow::anyhow!("Unexpected response: {other:?}"),
        }
    }

    // ---- identity ----

    /// The identity of the local node this client is connected to.
    pub async fn identity(&self) -> anyhow::Result<IdentityId> {
        match self.request(NodeCommand::Identity).await? {
            NodeResponse::Identity { identity_id } => Ok(identity_id),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Exports the identity key as an encrypted, transferable envelope. The
    /// same passphrase must be supplied again on [`Self::import_identity`].
    pub async fn export_identity(&self, passphrase: String) -> anyhow::Result<Vec<u8>> {
        match self
            .request(NodeCommand::ExportIdentity { passphrase })
            .await?
        {
            NodeResponse::IdentityExported { bytes } => Ok(bytes),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Imports an identity key previously exported via
    /// [`Self::export_identity`]. The imported identity is written to disk;
    /// the node must be restarted for it to take effect. `overwrite` must be
    /// `true` to replace an existing identity (the old key is backed up).
    pub async fn import_identity(
        &self,
        bytes: Vec<u8>,
        passphrase: String,
        overwrite: bool,
    ) -> anyhow::Result<IdentityId> {
        match self
            .request(NodeCommand::ImportIdentity {
                bytes,
                passphrase,
                overwrite,
            })
            .await?
        {
            NodeResponse::IdentityImported { identity_id } => Ok(identity_id),
            other => Err(Self::unexpected(other)),
        }
    }

    // ---- storage ----

    /// Stores data locally as a signed object, owned by the node's identity.
    pub async fn put(&self, data: Vec<u8>, name: Option<String>) -> anyhow::Result<ObjectId> {
        match self.request(NodeCommand::Put { data, name }).await? {
            NodeResponse::ObjectCreated { id } => Ok(id),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Records a human-friendly name for a stored object, so it shows up in
    /// listings and can be resolved id-free. The name lives *alongside* the
    /// object (a sidecar file), never inside the signed bundle.
    pub async fn set_name(&self, id: ObjectId, name: impl Into<String>) -> anyhow::Result<()> {
        match self
            .request(NodeCommand::SetName {
                id,
                name: name.into(),
            })
            .await?
        {
            NodeResponse::NameSet => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    pub async fn put_object(
        &self,
        data: Vec<u8>,
        object_type: ObjectType,
        name: Option<String>,
    ) -> anyhow::Result<ObjectId> {
        match self
            .request(NodeCommand::PutObject {
                data,
                object_type,
                name,
            })
            .await?
        {
            NodeResponse::ObjectCreated { id } => Ok(id),
            other => Err(Self::unexpected(other)),
        }
    }

    pub async fn put_file(&self, name: &str, data: Vec<u8>) -> anyhow::Result<ObjectId> {
        let response = self
            .request(NodeCommand::PutObject {
                data,
                object_type: ObjectType::Blob,
                name: Some(name.to_string()),
            })
            .await?;
        match response {
            NodeResponse::ObjectCreated { id } => Ok(id),
            NodeResponse::Error { message } => Err(anyhow::anyhow!(message)),
            _ => Err(anyhow::anyhow!("unexpected response")),
        }
    }

    /// Reads a locally stored object.
    pub async fn get(&self, id: ObjectId) -> anyhow::Result<Object> {
        match self.request(NodeCommand::Get { id }).await? {
            NodeResponse::Object { object } => Ok(object),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Lists all objects held in local storage.
    pub async fn list(&self) -> anyhow::Result<Vec<ObjectInfo>> {
        match self.request(NodeCommand::List).await? {
            NodeResponse::Objects { objects } => Ok(objects),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Exports a locally stored object as a portable, self-verifying bundle.
    pub async fn export(&self, id: ObjectId) -> anyhow::Result<ExportBundle> {
        match self.request(NodeCommand::Export { id }).await? {
            NodeResponse::Exported { bundle } => Ok(bundle),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Imports an object bundle (e.g. one received from a peer) into local storage.
    pub async fn import(&self, bundle: ExportBundle) -> anyhow::Result<()> {
        match self.request(NodeCommand::Import { bundle }).await? {
            NodeResponse::Imported => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    // ---- network ----

    /// Peers currently connected to the node's libp2p swarm.
    pub async fn peers(&self) -> anyhow::Result<Vec<PeerInfo>> {
        match self.request(NodeCommand::Peers).await? {
            NodeResponse::Peers { peers } => Ok(peers),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Dials a peer directly at the given multiaddr
    /// (e.g. `/ip4/1.2.3.4/tcp/4001/p2p/<peer id>`).
    pub async fn dial(&self, addr: impl Into<String>) -> anyhow::Result<()> {
        match self
            .request(NodeCommand::Dial { addr: addr.into() })
            .await?
        {
            NodeResponse::Dialed => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Requests a circuit reservation through a relay so peers behind other
    /// NATs can reach this node, enabling hole punching (dcutr) to upgrade
    /// to a direct connection.
    pub async fn listen_via_relay(&self, relay_addr: impl Into<String>) -> anyhow::Result<()> {
        match self
            .request(NodeCommand::ListenViaRelay {
                relay_addr: relay_addr.into(),
            })
            .await?
        {
            NodeResponse::ListeningViaRelay => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Lists accepted relay circuit reservations. Use this to confirm a
    /// `listen_via_relay` request actually succeeded (and to get the
    /// resulting dialable `/p2p-circuit` addresses) before asking others to
    /// dial you through that relay.
    pub async fn relay_reservations(&self) -> anyhow::Result<Vec<RelayReservationInfo>> {
        match self.request(NodeCommand::RelayReservations).await? {
            NodeResponse::RelayReservations { reservations } => Ok(reservations),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Announces on the DHT that this node holds the given object, so other
    /// peers can discover it via [`Self::find_providers`].
    pub async fn announce(&self, id: ObjectId) -> anyhow::Result<()> {
        match self.request(NodeCommand::Announce { id }).await? {
            NodeResponse::Announced => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Finds peers that have announced the given object.
    pub async fn find_providers(&self, id: ObjectId) -> anyhow::Result<Vec<String>> {
        match self.request(NodeCommand::FindProviders { id }).await? {
            NodeResponse::Providers { peer_ids } => Ok(peer_ids),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Signs and publishes a pointer from this node's `(identity, name)` to
    /// `manifest`, so peers can resolve the latest version of an app
    /// published under `name` without needing a fresh manifest id out of
    /// band every time it's republished. Overwrites any pointer previously
    /// published under the same name.
    pub async fn publish_app_pointer(
        &self,
        name: impl Into<String>,
        manifest: ObjectId,
    ) -> anyhow::Result<()> {
        match self
            .request(NodeCommand::PublishAppPointer {
                name: name.into(),
                manifest,
            })
            .await?
        {
            NodeResponse::AppPointerPublished => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Resolves the latest manifest id published by `owner` under `name`,
    /// verifying the pointer's signature actually belongs to `owner`.
    /// Returns `None` if no (verifiable) pointer is found.
    pub async fn resolve_app_pointer(
        &self,
        owner: IdentityId,
        name: impl Into<String>,
    ) -> anyhow::Result<Option<ObjectId>> {
        match self
            .request(NodeCommand::ResolveAppPointer {
                owner,
                name: name.into(),
            })
            .await?
        {
            NodeResponse::AppPointer {
                record: Some(record),
            } if record.verify() => Ok(Some(record.manifest)),
            NodeResponse::AppPointer { .. } => Ok(None),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Signs and publishes a user record `(owner, name)` → `ObjectId`, using
    /// the Runtime's cache-aware pointer layer (local record cache + DHT).
    pub async fn publish_pointer(
        &self,
        name: impl Into<String>,
        target: ObjectId,
    ) -> anyhow::Result<()> {
        match self
            .request(NodeCommand::PublishPointer {
                name: name.into(),
                target,
            })
            .await?
        {
            NodeResponse::PointerPublished => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Resolves a `(owner, name)` user record through the Runtime's
    /// cache-aware pointer layer. Returns the latest verified
    /// `AppPointerRecord`, or `None` if nothing verifiable is found.
    pub async fn resolve_pointer(
        &self,
        owner: IdentityId,
        name: impl Into<String>,
    ) -> anyhow::Result<Option<AppPointerRecord>> {
        match self
            .request(NodeCommand::ResolvePointer {
                owner,
                name: name.into(),
            })
            .await?
        {
            NodeResponse::Pointer { record } => Ok(record),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Stores a new `Profile` version and repoints the node's
    /// `(owner, "profile")` record at it. Returns the new object id.
    pub async fn save_profile(&self, profile: &Profile) -> anyhow::Result<ObjectId> {
        match self
            .request(NodeCommand::SaveProfile {
                profile: profile.clone(),
            })
            .await?
        {
            NodeResponse::ProfileSaved { id } => Ok(id),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Loads the node's latest `Profile` from the shared store.
    pub async fn load_profile(&self) -> anyhow::Result<Option<Profile>> {
        match self.request(NodeCommand::LoadProfile).await? {
            NodeResponse::Profile { profile } => Ok(profile),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Claims a globally unique username for the node's identity, so other
    /// peers can discover and address this node by name (see
    /// [`Self::resolve_username`]). Republishing under a new name re-claims it.
    pub async fn claim_username(&self, username: impl Into<String>) -> anyhow::Result<()> {
        match self
            .request(NodeCommand::ClaimUsername {
                username: username.into(),
            })
            .await?
        {
            NodeResponse::UsernameClaimed => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// The username currently claimed by the node's identity, if any.
    pub async fn show_username(&self) -> anyhow::Result<Option<String>> {
        match self.request(NodeCommand::ShowUsername).await? {
            NodeResponse::Username { username } => Ok(username),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Starts a publish/serve session for the app whose manifest is
    /// `app_id` with the Canopee edge at `edge_addr` (a `/p2p/...` multiaddr),
    /// so the edge routes `<app-hash>.<base>/...` HTTP traffic to this node.
    /// The session stays live until [`Self::stop_serve_session`] is called.
    /// Returns the app id and the session's public root URL.
    pub async fn start_serve_session(
        &self,
        edge_addr: &str,
        app_id: &ObjectId,
    ) -> anyhow::Result<(String, String)> {
        match self
            .request(NodeCommand::StartServeSession {
                edge_addr: edge_addr.to_string(),
                app_id: app_id.clone(),
            })
            .await?
        {
            NodeResponse::ServeSessionStarted { app_id, root_url } => Ok((app_id, root_url)),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Deregisters the node's current serve session with the edge and stops
    /// its heartbeat. No-op when no session is running.
    pub async fn stop_serve_session(&self) -> anyhow::Result<()> {
        match self.request(NodeCommand::StopServeSession).await? {
            NodeResponse::ServeSessionStopped => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// This machine's device `PeerId` (from its per-device key) and human
    /// name.
    pub async fn device(&self) -> anyhow::Result<(String, String)> {
        match self.request(NodeCommand::Device).await? {
            NodeResponse::Device {
                peer_id,
                device_name,
            } => Ok((peer_id, device_name)),
            other => Err(Self::unexpected(other)),
        }
    }

    /// The devices currently carrying the node's identity, via the
    /// `(owner, "devices")` record.
    pub async fn device_list(&self) -> anyhow::Result<Vec<DeviceInfo>> {
        match self.request(NodeCommand::DeviceList).await? {
            NodeResponse::DeviceList { devices } => Ok(devices),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Resolves which device `PeerId` to dial to reach `owner`, via the
    /// owner's `(owner, "devices")` list. `None` when the owner has no
    /// registered device.
    pub async fn resolve_owner_device(
        &self,
        owner: &IdentityId,
    ) -> anyhow::Result<Option<String>> {
        match self
            .request(NodeCommand::ResolveOwnerDevice { owner: owner.clone() })
            .await?
        {
            NodeResponse::OwnerDevice { peer_id } => Ok(peer_id),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Reverse-resolves a friendly username to its canonical owner identity
    /// via the DHT registry (spoof-verified against the owner's signed
    /// record). Returns `None` if the name is unclaimed or unverified.
    pub async fn resolve_username(
        &self,
        username: impl Into<String>,
    ) -> anyhow::Result<Option<IdentityId>> {
        match self
            .request(NodeCommand::ResolveUsername {
                username: username.into(),
            })
            .await?
        {
            NodeResponse::UsernameOwner { owner } => Ok(owner),
            other => Err(Self::unexpected(other)),
        }
    }

    // ---- LAN device pairing ----

    /// Starts a pairing session on THIS device (the new device): mints a
    /// fresh 12-char pairing code + one-time session id and returns the QR
    /// data to display/print. The source device's user then reads the code
    /// back as their explicit approval.
    pub async fn pair_initiate(&self) -> anyhow::Result<PairingQrData> {
        match self.request(NodeCommand::InitiatePairing).await? {
            NodeResponse::PairingQr { qr } => Ok(qr),
            other => Err(Self::unexpected(other)),
        }
    }

    /// The counterpart to [`Self::pair_initiate`], run on the device already
    /// carrying the identity: verifies the user-typed `code` against the QR
    /// data, encrypts the identity + user records, and delivers them to the
    /// new device over the LAN. Returns the new device's status message.
    pub async fn pair_complete(
        &self,
        qr: PairingQrData,
        code: String,
    ) -> anyhow::Result<String> {
        match self
            .request(NodeCommand::CompletePairing { qr, code })
            .await?
        {
            NodeResponse::PairingComplete { message } => Ok(message),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Refreshes this node's user records (profile, contacts, devices) from
    /// the network, optionally dialing `peer_id` first. Last-writer-wins: a
    /// newer signed pointer on the DHT replaces the local cache.
    pub async fn sync_with_peer(&self, peer_id: impl Into<String>) -> anyhow::Result<SyncResult> {
        match self
            .request(NodeCommand::SyncFromPeer {
                peer_id: peer_id.into(),
            })
            .await?
        {
            NodeResponse::SyncComplete { result } => Ok(result),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Refreshes this node's user records from every device in its
    /// `(owner, "devices")` list.
    pub async fn sync_with_all_devices(&self) -> anyhow::Result<SyncResult> {
        match self.request(NodeCommand::SyncDeviceList).await? {
            NodeResponse::SyncComplete { result } => Ok(result),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Stores a new `ContactList` snapshot and repoints the node's
    /// `(owner, "contacts")` record at it. Returns the new object id.
    pub async fn save_contact_list(&self, list: &ContactList) -> anyhow::Result<ObjectId> {
        match self
            .request(NodeCommand::SaveContactList {
                list: list.clone(),
            })
            .await?
        {
            NodeResponse::ContactListSaved { id } => Ok(id),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Loads the node's latest `ContactList` from the shared store.
    pub async fn load_contact_list(&self) -> anyhow::Result<Option<ContactList>> {
        match self.request(NodeCommand::LoadContactList).await? {
            NodeResponse::ContactList { list } => Ok(list),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Stores a new `HomeIndex` version and repoints the node's
    /// `(owner, "home")` record at it. Returns the new object id.
    pub async fn save_home_index(&self, index: &HomeIndex) -> anyhow::Result<ObjectId> {
        match self
            .request(NodeCommand::SaveHomeIndex {
                index: index.clone(),
            })
            .await?
        {
            NodeResponse::HomeIndexSaved { id } => Ok(id),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Loads the node's latest `HomeIndex` from the shared store.
    pub async fn load_home_index(&self) -> anyhow::Result<Option<HomeIndex>> {
        match self.request(NodeCommand::LoadHomeIndex).await? {
            NodeResponse::HomeIndex { index } => Ok(index),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Flips one home entry's `shared` flag — the explicit "share this on the
    /// network" / "stop sharing" action. Sharing announces the entry's object
    /// as a DHT provider and starts serving it; unsharing withdraws both.
    /// Returns the new index object id.
    pub async fn set_home_entry_shared(
        &self,
        name: impl Into<String>,
        shared: bool,
    ) -> anyhow::Result<ObjectId> {
        match self
            .request(NodeCommand::SetHomeEntryShared {
                name: name.into(),
                shared,
            })
            .await?
        {
            NodeResponse::HomeIndexSaved { id } => Ok(id),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Shares a stored object under `name`: upserts a `shared: true` home
    /// entry (creating the index on first use) and announces the object as a
    /// DHT provider. The one-call "put this file on the network" action.
    pub async fn share_object(
        &self,
        name: impl Into<String>,
        object: ObjectId,
        app: Option<String>,
    ) -> anyhow::Result<ObjectId> {
        match self
            .request(NodeCommand::ShareObject {
                name: name.into(),
                object,
                app,
            })
            .await?
        {
            NodeResponse::HomeIndexSaved { id } => Ok(id),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Stops sharing the home entry named `name` (announces withdrawn, object
    /// no longer served). Shorthand for `set_home_entry_shared(name, false)`.
    pub async fn unshare(&self, name: impl Into<String>) -> anyhow::Result<ObjectId> {
        self.set_home_entry_shared(name, false).await
    }

    // ---- capabilities ("who is allowed to do what") ----

    /// Issues a signed capability on behalf of the node's identity: grants
    /// `subject` `permissions` over `resource`, optionally expiring at
    /// `expires_at` (unix seconds). The signed grant is stored locally and
    /// returned so it can be hand-delivered to the subject out of band.
    pub async fn grant_capability(
        &self,
        subject: IdentityId,
        resource: Resource,
        permissions: Vec<Permission>,
        expires_at: Option<u64>,
    ) -> anyhow::Result<Capability> {
        match self
            .request(NodeCommand::GrantCapability {
                subject,
                resource,
                permissions,
                expires_at,
            })
            .await?
        {
            NodeResponse::CapabilityGranted { capability } => Ok(capability),
            other => Err(Self::unexpected(other)),
        }
    }

    /// The capability grants the node's identity has issued, from the
    /// `(owner, "capabilities")` record (revocation state included). `None`
    /// until the first grant.
    pub async fn list_capabilities(&self) -> anyhow::Result<Option<CapabilityIndex>> {
        match self.request(NodeCommand::ListCapabilities).await? {
            NodeResponse::Capabilities { index } => Ok(index),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Marks a previously issued capability as revoked (republishes the
    /// issuer's index). The subject learns of the revocation when an app next
    /// verifies the grant against the issuer.
    pub async fn revoke_capability(&self, id: &CapabilityId) -> anyhow::Result<()> {
        match self
            .request(NodeCommand::RevokeCapability {
                id: id.clone(),
            })
            .await?
        {
            NodeResponse::CapabilityRevoked => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Verifies a presented capability end to end: signature + content id +
    /// issuer derivation, time window, and (when reachable) the issuer's
    /// revocation index. Returns `(valid, reason)`.
    pub async fn check_capability(
        &self,
        capability: &Capability,
    ) -> anyhow::Result<(bool, String)> {
        match self
            .request(NodeCommand::CheckCapability {
                capability: capability.clone(),
            })
            .await?
        {
            NodeResponse::CapabilityCheck { valid, reason } => Ok((valid, reason)),
            other => Err(Self::unexpected(other)),
        }
    }

    /// The issuer-side authorization check (vision §5.5): does `subject`
    /// currently hold an unrevoked, unexpired grant from the node's identity
    /// of `permission` on `resource`? Run this before serving a resource.
    pub async fn check_access(
        &self,
        subject: &IdentityId,
        permission: Permission,
        resource: &Resource,
    ) -> anyhow::Result<bool> {
        match self
            .request(NodeCommand::CheckAccess {
                subject: subject.clone(),
                permission,
                resource: resource.clone(),
            })
            .await?
        {
            NodeResponse::AccessAllowed { allowed } => Ok(allowed),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Fetches an object directly from a specific peer (typically one found
    /// via [`Self::find_providers`]) and stores it locally.
    pub async fn fetch_object(
        &self,
        peer_id: impl Into<String>,
        id: ObjectId,
    ) -> anyhow::Result<ExportBundle> {
        match self
            .request(NodeCommand::FetchObject {
                peer_id: peer_id.into(),
                id,
            })
            .await?
        {
            NodeResponse::Exported { bundle } => Ok(bundle),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Publishes data to a gossipsub topic. Peers must be connected and
    /// subscribed to the same topic to receive it.
    pub async fn publish(&self, topic: impl Into<String>, data: Vec<u8>) -> anyhow::Result<()> {
        match self
            .request(NodeCommand::Publish {
                topic: topic.into(),
                data,
            })
            .await?
        {
            NodeResponse::Published => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Subscribes to a gossipsub topic. The returned [`Subscription`] streams
    /// messages until dropped or the node disconnects.
    pub async fn subscribe(&self, topic: impl Into<String>) -> anyhow::Result<Subscription> {
        Subscription::open(&self.node, topic.into()).await
    }

    /// Requests the local node to shut down.
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        match self.request(NodeCommand::Shutdown).await? {
            NodeResponse::ShutdownAccepted => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }
}
