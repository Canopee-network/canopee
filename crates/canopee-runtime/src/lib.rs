mod state;
use canopee_config::Config;
use canopee_identity::{DeviceKey, Identity, IdentityId};
use canopee_network::{
    CanopeePairingRequest, CanopeePairingResponse, InboundPairing, Multiaddr, NetworkManager,
    ObjectProvider, PeerId,
};
use canopee_protocol::{
    PairingData, PairingPayload, PairingQrData, PairingRecord, SyncResult,
};
use canopee_storage::{
    AppPointerRecord, Cache, CacheIndex, ContactList, DeviceEntry, DeviceList, Export,
    ExportBundle, HomeIndex, Object, ObjectId, ObjectInfo, ObjectType, Profile, UsernameRecord,
    Verify, DEVICE_REGISTRY_PREFIX, RECORD_CONTACTS, RECORD_DEVICES, RECORD_HOME, RECORD_PROFILE,
    RECORD_USERNAME, USERNAME_REGISTRY_PREFIX, Storage,
};
use state::NodeState;
use std::path::PathBuf;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::RwLock;

/// Normalizes a claimed username: trims whitespace and lowercases. Rejects
/// empty or whitespace-only names.
fn normalize_username(username: &str) -> anyhow::Result<String> {
    let normalized = username.trim().to_lowercase();
    if normalized.is_empty() {
        anyhow::bail!("username must not be empty");
    }
    if normalized
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
    {
        anyhow::bail!(
            "username \"{username}\" contains invalid characters: only letters, digits, '_', '-', '.' allowed"
        );
    }
    Ok(normalized)
}

/// Extracts the embedded libp2p `PeerId` from a canonical
/// `canopee://identity/<peer-id>` identity string. `None` if the string
/// isn't in that form or the peer id doesn't parse.
fn owner_peer_id(owner: &IdentityId) -> Option<PeerId> {
    owner
        .to_string()
        .strip_prefix("canopee://identity/")
        .and_then(|rest| rest.split('/').next())
        .and_then(|s| s.parse().ok())
}

#[derive(Clone)]
pub struct Runtime {
    pub config: Config,
    pub identity: Arc<Identity>,
    /// The device keypair backing this machine's network `PeerId` — distinct
    /// from `identity` (the *person*), so several devices of one identity can
    /// be online at once without colliding. Never leaves this device.
    pub device_key: Arc<DeviceKey>,
    pub storage: Arc<Storage>,
    pub network: NetworkManager,
    pub cache: Arc<Cache>,
    state: Arc<RwLock<NodeState>>,
    shared: SharedSet,
    /// The in-flight LAN pairing session on THIS device (the new device):
    /// the 12-char code + one-time session id minted by `initiate_pairing`,
    /// held only in memory until the inbound request arrives. Replaced (not
    /// stored) on every new `initiate_pairing`; discarded on accept, restart,
    /// or when the node shuts down.
    pairing: Arc<RwLock<Option<OwnPairing>>>,
}

/// One in-flight pairing session on the *new* device. `session_id` is also
/// carried in the `PairingQrData` (and on the wire), while `code` is held
/// only here — it is the shared secret that decrypts the incoming payload.
struct OwnPairing {
    session_id: String,
    code: String,
}

/// The set of objects this node explicitly serves to the network. In-memory
/// by design: DHT provider records are themselves ephemeral (re-announced
/// per session), so the servable set matches their lifetime. `shared: true`
/// home entries are the exception — they are re-seeded from the persisted
/// `HomeIndex` every time the runtime opens, so a user's explicit shares
/// survive restarts.
///
/// "Nothing is shared-by-default": an owned object is only served once it
/// lands in this set (via `Runtime::announce` or a `shared: true` home
/// entry). Cached objects (imported from other peers) don't need to be in
/// the set — re-hosting them is the distributed cache, and they were public
/// on the network already.
#[derive(Clone, Default)]
struct SharedSet {
    ids: Arc<RwLock<std::collections::HashSet<String>>>,
}

impl SharedSet {
    async fn contains(&self, id: &ObjectId) -> bool {
        self.ids.read().await.contains(&id.0)
    }

    async fn insert(&self, id: &ObjectId) {
        self.ids.write().await.insert(id.0.clone());
    }

    async fn remove(&self, id: &ObjectId) {
        self.ids.write().await.remove(&id.0);
    }
}

struct StorageObjectProvider {
    storage: Arc<Storage>,
    cache: Arc<Cache>,
    shared: SharedSet,
}

#[async_trait::async_trait]
impl ObjectProvider for StorageObjectProvider {
    async fn get_object(&self, id: &ObjectId) -> Option<ExportBundle> {
        let object = self.storage.get_verified(id).await.ok()?;
        // Cached (imported from another peer) objects are always re-served —
        // that re-hosting *is* the distributed cache. Touch the timestamp so
        // LRU eviction sees the "someone is actually using my cached copy"
        // signal.
        if self.cache.is_cached(id).await {
            self.cache.touch_served(id).await;
            return object.export().ok();
        }
        // Everything else (owned objects above all: profiles, contact lists,
        // home indices, private files) is served only after an explicit
        // share action put it in the shared set.
        if !self.shared.contains(id).await {
            return None;
        }
        object.export().ok()
    }
}

impl Runtime {
    pub async fn open() -> anyhow::Result<Self> {
        Self::open_with_config(Config::new()).await
    }

    /// Opens a `Runtime` rooted at an explicit directory (see
    /// `Config::with_root`), so embedded apps can each own an isolated
    /// identity/storage without sharing `~/.canopee`.
    pub async fn open_with_root(root: PathBuf) -> anyhow::Result<Self> {
        Self::open_with_config(Config::with_root(root)).await
    }

    /// Opens a `Runtime` for an embedded desktop app: everything app-specific
    /// (network state, cache, socket, exports) is rooted at `app_root`, while
    /// the *identity*, the *user object store*, and the *record cache* stay at
    /// the shared `~/.canopee` user root.
    ///
    /// This is the "state is per-app, data is per-user" split: every app the
    /// user runs via this entrypoint shares one identity and one set of
    /// objects, so contacts, files, pictures, and profile data follow them
    /// across apps on the same machine (and, via the records, to new devices).
    ///
    /// mDNS is disabled by default: several apps may be open at once, and a
    /// shared identity must never be re-announced over multicast by each
    /// swarm (peers still discover each other via Kademlia/bootstrap + dialing).
    pub async fn open_with_user_root(app_root: PathBuf) -> anyhow::Result<Self> {
        Self::open_with_config(Config::new().with_app_root(app_root).with_mdns(false)).await
    }

    pub async fn open_with_config(config: Config) -> anyhow::Result<Self> {
        let root = config.home_dir();
        tokio::fs::create_dir_all(&root).await?;
        let identity_dir = config.identity_path();
        tokio::fs::create_dir_all(&identity_dir).await?;
        let identity_path = identity_dir.join("identity.key");
        let passphrase: Option<String> = std::env::var_os("CANOPEE_IDENTITY_PASS")
            .and_then(|p| p.into_string().ok());
        let identity = match passphrase.as_deref() {
            Some(pass) => {
                let path_str = identity_path.to_str().unwrap();
                if !tokio::fs::try_exists(&identity_path).await.unwrap_or(false) {
                    Identity::create_encrypted(path_str, pass).await?
                } else {
                    let (id, encrypted_at_rest) =
                        Identity::load_encrypted(path_str, pass).await?;
                    if !encrypted_at_rest {
                        eprintln!(
                            "WARNING: CANOPEE_IDENTITY_PASS is set but the identity key \
                             at {} is not encrypted at rest. To encrypt it, delete the \
                             key file and restart with CANOPEE_IDENTITY_PASS set.",
                            identity_path.display()
                        );
                    }
                    id
                }
            }
            // `create_if_absent` makes the shared user root race-safe: whichever
            // app runs first creates the identity, everyone else adopts the same
            // one — the "one identity per person" guarantee.
            None => Identity::create_if_absent(identity_path.to_str().unwrap()).await?,
        };
        let state_path = config.state_path();
        tokio::fs::create_dir_all(state_path.parent().unwrap()).await?;

        let state = match tokio::fs::read(&state_path).await {
            Ok(bytes) => bincode::deserialize(&bytes)?,
            Err(_) => {
                let state = NodeState {
                    identity: identity.id().clone(),
                    created_at: OffsetDateTime::now_utc().unix_timestamp() as u64,
                    last_started_at: None,
                    started: false,
                    version: 1,
                    peers: vec![],
                };
                let bytes = bincode::serialize(&state)?;
                tokio::fs::write(&state_path, bytes).await?;

                state
            }
        };
        let storage_path = config.storage_path();
        tokio::fs::create_dir_all(&storage_path).await?;
        let storage = Arc::new(Storage::new(storage_path.to_str().unwrap()));
        let identity = Arc::new(identity);

        let cache_path = config.cache_path();
        let cache_index = CacheIndex::load(&cache_path).await;
        let cache = Arc::new(Cache::new(cache_path, identity.id().clone(), cache_index));

        let listen_addr: Multiaddr = config.listen_addr().parse()?;
        let shared = SharedSet::default();
        let object_provider = Arc::new(StorageObjectProvider {
            storage: storage.clone(),
            cache: cache.clone(),
            shared: shared.clone(),
        });
        // Device key: the *machine's* network identity, separate from the
        // person's `identity.key`. Created once per user root; the
        // race-safe `load_or_create` keeps two apps provisioning the same
        // root from minting two device identities.
        let device_path = config.device_key_path();
        tokio::fs::create_dir_all(device_path.parent().unwrap()).await?;
        let device_key = Arc::new(
            DeviceKey::load_or_create(device_path.to_str().unwrap(), &Self::default_device_name())
                .await?,
        );
        let network = NetworkManager::new(
            device_key.keypair(),
            listen_addr,
            object_provider,
            config.mdns_enabled(),
        )?;

        let runtime = Self {
            config,
            identity,
            device_key,
            storage,
            network,
            cache,
            state: Arc::new(RwLock::new(state)),
            shared,
            pairing: Arc::new(RwLock::new(None)),
        };

        // Seed the serving set from the persisted home index: entries the
        // user previously marked `shared: true` stay shared across restarts.
        // Best-effort — a DHT failure at startup (offline, no bootstrap yet)
        // must not undo a share or fail the open.
        if let Ok(Some(index)) = runtime.load_home_index().await {
            runtime.reconcile_shared(None, Some(&index)).await;
        }

        // Re-serve the public user records (profile, username) the user
        // previously published, so peers can keep resolving them after a
        // restart (the shared set and DHT provider records are per-session).
        runtime.reshare_public_user_records().await;

        // Register this device in the shared `(owner, "devices")` list and
        // the device→identity registry. Best-effort: an offline start must
        // not fail the open, and the durable local pieces (object, pointer,
        // registry cache) are what let other devices resolve us.
        runtime.register_device().await;

        // The DHT copies of those records are fire-and-forget at startup,
        // when the swarm has no peers yet — schedule one re-publication once
        // the network is reachable so peers can actually resolve this device.
        runtime.schedule_device_publish();

        // Serve the LAN device-pairing protocol for the lifetime of this
        // runtime: inbound `/canopee/pairing/1.0.0` requests are decrypted
        // against the in-memory pairing session and imported to disk.
        runtime.spawn_pairing_handler();

        // Keep paired devices in step without user action: refresh the
        // identity-scoped user records from the DHT on an interval, but only
        // when the node is online and has at least one connected peer.
        runtime.spawn_periodic_sync();

        Ok(runtime)
    }

    /// The human-friendly name this device announces: `CANOPEE_DEVICE_NAME`
    /// when set, otherwise the machine's hostname.
    fn default_device_name() -> String {
        std::env::var("CANOPEE_DEVICE_NAME")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                hostname::get()
                    .map(|h| h.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "canopee-device".into())
            })
    }

    /// Re-announces the objects the local `(owner, "profile")`, `(owner,
    /// "username")`, and `(owner, "devices")` records point at, if any. Reads
    /// the local record cache only (no DHT lookups) so startup stays fast;
    /// failures are logged and skipped.
    async fn reshare_public_user_records(&self) {
        for name in [RECORD_PROFILE, RECORD_USERNAME, RECORD_DEVICES] {
            let key = AppPointerRecord::key(self.identity.id(), name);
            let path = self
                .config
                .records_path()
                .join(format!("{}.record", hex::encode(&key)));
            let Ok(bytes) = tokio::fs::read(&path).await else {
                continue;
            };
            let Ok(record) = bincode::deserialize::<AppPointerRecord>(&bytes) else {
                continue;
            };
            if !record.verify() {
                continue;
            }
            if self.storage.exists(&record.manifest).await {
                if let Err(e) = self.announce(record.manifest.clone()).await {
                    tracing::warn!("re-sharing {name} record failed: {e}");
                }
            }
        }
    }

    pub async fn mark_started(&self) -> anyhow::Result<()> {
        {
            let mut state = self.state.write().await;

            state.started = true;
            state.last_started_at = Some(time::OffsetDateTime::now_utc().unix_timestamp() as u64);
        }
        self.save_state().await?;

        Ok(())
    }
    pub async fn mark_stopped(&self) -> anyhow::Result<()> {
        {
            let mut state = self.state.write().await;
            state.started = false;
        }
        self.save_state().await?;

        Ok(())
    }

    async fn save_state(&self) -> anyhow::Result<()> {
        let bytes = {
            let state = self.state.read().await;
            bincode::serialize(&*state)?
        };
        let path = self.config.state_path();
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(tmp, path).await?;

        Ok(())
    }

    pub async fn status(&self) -> NodeState {
        self.state.read().await.clone()
    }

    pub fn export_path(&self) -> PathBuf {
        self.config.export_path()
    }

    pub fn node_socket_path(&self) -> PathBuf {
        self.config.node_socket_path()
    }

    pub async fn export_to_file(&self, id: &ObjectId) -> anyhow::Result<ExportBundle> {
        let bundle = self.export(id).await?;
        let export_dir = self.config.export_path();
        tokio::fs::create_dir_all(&export_dir).await?;
        let path = export_dir.join(format!("{}.canopee", id));
        let bytes = bincode::serialize(&bundle)?;
        tokio::fs::write(&path, bytes).await?;

        Ok(bundle)
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Returns the identity private key as an encrypted, transferable
    /// envelope (see `Identity::export_encrypted`) — the "move my identity to
    /// another device" action. `passphrase` encrypts the exported bytes; the
    /// same passphrase is needed on import.
    pub fn export_identity(&self, passphrase: &str) -> anyhow::Result<Vec<u8>> {
        self.identity.export_encrypted(passphrase)
    }

    /// Imports an identity key previously exported via
    /// [`Self::export_identity`] and persists it to this device's identity
    /// file. The live runtime is already bound to its current key (storage
    /// ownership, the running swarm), so the imported identity only takes
    /// effect after the node restarts — this method's job is the durable,
    /// non-destructive write.
    ///
    /// Rules:
    /// - The exported bytes are fully validated (passphrase + integrity +
    ///   keypair parse) before anything touches disk.
    /// - If the imported key is identical to the one already loaded, this is a
    ///   no-op: the node already owns the identity, no restart needed.
    /// - An existing identity file is never overwritten silently: `overwrite`
    ///   must be set, and the old key is first backed up to
    ///   `identity.key.bak-<timestamp>`.
    /// - The at-rest format follows the node's configured policy: if
    ///   `CANOPEE_IDENTITY_PASS` is set the imported key is stored encrypted
    ///   at rest under that passphrase, otherwise as plaintext — so the
    ///   exported transfer passphrase and the node's at-rest passphrase stay
    ///   independent.
    pub async fn import_identity(
        &self,
        bytes: &[u8],
        transfer_passphrase: &str,
        overwrite: bool,
    ) -> anyhow::Result<IdentityId> {
        let imported = Identity::import_from_encrypted(bytes, transfer_passphrase)?;
        if imported.id() == self.identity.id() {
            return Ok(self.identity.id().clone());
        }

        let identity_path = self.config.identity_path().join("identity.key");
        let exists = tokio::fs::try_exists(&identity_path).await.unwrap_or(false);
        if exists && !overwrite {
            anyhow::bail!(
                "an identity already exists at {}; pass `overwrite: true` to replace it \
                 (the existing key is backed up, never deleted)",
                identity_path.display()
            );
        }
        if exists {
            let backup = self
                .config
                .identity_path()
                .join(format!(
                    "identity.key.bak-{}",
                    OffsetDateTime::now_utc().unix_timestamp()
                ));
            tokio::fs::copy(&identity_path, &backup).await?;
        }

        // Persist in the same at-rest format the node expects on startup
        // (mirrors the `CANOPEE_IDENTITY_PASS` branch of `open_with_config`).
        let passphrase: Option<String> = std::env::var_os("CANOPEE_IDENTITY_PASS")
            .and_then(|p| p.into_string().ok());
        let bytes_to_write = match passphrase.as_deref() {
            Some(pass) => imported.export_encrypted(pass)?,
            None => imported.export_bytes()?,
        };

        let tmp = self.config.identity_path().join("identity.key.tmp");
        tokio::fs::write(&tmp, bytes_to_write).await?;
        tokio::fs::rename(&tmp, &identity_path).await?;

        tracing::info!(
            "imported identity {} -> {} (restart the node to adopt it)",
            self.identity.id(),
            imported.id()
        );
        Ok(imported.id().clone())
    }

    pub async fn put(&self, data: Vec<u8>, name: Option<String>) -> anyhow::Result<ObjectId> {
        let object = Object::new(&self.identity, data, ObjectType::Blob);
        let id = object.id.clone();
        self.storage.put_verified(&object).await?;
        if let Some(name) = name {
            self.storage.set_name(&id, &name).await?;
        }

        Ok(id)
    }

    pub async fn put_object(
        &self,
        data: Vec<u8>,
        object_type: ObjectType,
        name: Option<String>,
    ) -> anyhow::Result<Object> {
        let object = Object::new(&self.identity(), data, object_type);

        self.storage.put_verified(&object).await?;
        if let Some(name) = name {
            self.storage.set_name(&object.id, &name).await?;
        }

        Ok(object)
    }

    pub async fn get(&self, id: &ObjectId) -> anyhow::Result<Object> {
        let object = self.storage.get_verified(id).await?;

        Ok(object)
    }

    pub async fn list(&self) -> anyhow::Result<Vec<ObjectInfo>> {
        self.storage.list_objects().await
    }

    pub async fn export(&self, id: &ObjectId) -> anyhow::Result<ExportBundle> {
        let object = self.storage.get_verified(id).await?;
        let export_bundle = object.export()?;

        Ok(export_bundle)
    }

    pub async fn import(&self, export_bundle: ExportBundle) -> anyhow::Result<()> {
        let exists = self.storage.exists(&export_bundle.object.id).await;
        if exists {
            anyhow::bail!("Object already exists");
        }
        let is_cached = export_bundle.object.payload.owner != *self.identity.id();
        self.storage.import(&export_bundle.object).await?;
        if is_cached {
            self.cache.mark_cached(&export_bundle.object.id).await;
        }
        Ok(())
    }

    /// Deletes a single object from local storage, removes it from the cache
    /// index, and withdraws this node as a DHT provider of it. Used when a
    /// cached object is evicted.
    pub async fn evict(&self, id: &ObjectId) -> anyhow::Result<()> {
        self.storage.delete(id).await?;
        self.cache.remove(id).await;
        self.shared.remove(id).await;
        let _ = self.network.unannounce(id.clone()).await;
        Ok(())
    }

    /// Total bytes currently held by cached (non-owned) objects.
    pub async fn cached_bytes(&self) -> u64 {
        self.cache.total_cached_bytes(&self.storage).await
    }

    /// Cached objects ordered least-recently-served first.
    pub async fn lru_cached(&self) -> Vec<Object> {
        self.cache.lru_cached_objects(&self.storage).await
    }

    // ---- network sharing ("nothing is shared-by-default") ----

    /// Announces this node as a DHT provider of `id` and marks the object as
    /// shared, so the object-exchange protocol serves it to any peer that
    /// asks. Announcing IS the explicit "share this on the network" action
    /// for app objects (publish announces the manifest, entrypoint, and every
    /// asset). The network error (if any) is propagated, but the object is
    /// marked shared regardless — a peer that already knows the id may still
    /// fetch it directly.
    pub async fn announce(&self, id: ObjectId) -> anyhow::Result<()> {
        self.shared.insert(&id).await;
        self.network.announce(id).await
    }

    /// Marks an object as servable without announcing it on the DHT — peers
    /// who learn the id out of band can fetch it, but it isn't discoverable.
    pub async fn mark_shared(&self, id: &ObjectId) {
        self.shared.insert(id).await;
    }

    /// `true` if `id` is currently served to the network (explicitly shared,
    /// or cached from a peer).
    pub async fn is_shared(&self, id: &ObjectId) -> bool {
        self.shared.contains(id).await || self.cache.is_cached(id).await
    }

    // ---- user records ("state is per-app, data is per-user") ----

    /// Publishes the `(owner=self, name)` record pointing at `target`.
    ///
    /// The local record cache write is awaited — it is the authoritative
    /// same-machine view and must be durable before we return. The DHT put
    /// runs in the background: a kad `put_record` can legitimately take tens
    /// of seconds on an unhealthy or slow network (query timeout), and that
    /// latency must never stall a local publish.
    pub async fn publish_pointer(&self, name: &str, target: ObjectId) -> anyhow::Result<()> {
        let record = AppPointerRecord::sign(&self.identity, name, target.clone())?;
        let key = AppPointerRecord::key(&record.owner, name);
        let cache_path = self.config.records_path();
        tokio::fs::create_dir_all(&cache_path).await?;
        let path = cache_path.join(format!("{}.record", hex::encode(&key)));
        let bytes = bincode::serialize(&record)?;
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, &bytes).await?;
        tokio::fs::rename(&tmp, path).await?;
        // Fire-and-forget DHT publication: best-effort, logged on failure.
        let network = self.network.clone();
        tokio::spawn(async move {
            if let Err(e) = network.put_record(key, bytes).await {
                tracing::warn!("DHT put_record failed (record cached locally): {e}");
            }
        });
        Ok(())
    }

    /// How long a pointer resolution waits on the DHT before falling back to
    /// "not found". Bounds first-run lookups on a slow or unreachable network
    /// (a kad query can otherwise take its full ~60s timeout); healthy local
    /// lookups answer in milliseconds.
    const RESOLVE_DHT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    /// Resolves `(owner, name)` to the latest signed pointer. Checks the
    /// local record cache first (so same-machine apps agree instantly), then
    /// the DHT (bounded — see `RESOLVE_DHT_TIMEOUT`). Returns `None` when
    /// nothing verifiable is found for the key.
    pub async fn resolve_pointer(
        &self,
        owner: &IdentityId,
        name: &str,
    ) -> anyhow::Result<Option<AppPointerRecord>> {
        let key = AppPointerRecord::key(owner, name);
        let cache_path = self.config.records_path().join(format!("{}.record", hex::encode(&key)));
        let from_cache = tokio::fs::read(&cache_path).await.ok().and_then(|bytes| {
            let record: AppPointerRecord = bincode::deserialize(&bytes).ok()?;
            (record.owner == *owner && record.name == name && record.verify()).then_some(record)
        });
        if let Some(record) = from_cache {
            return Ok(Some(record));
        }
        // A DHT error or timeout (offline, no bootstrap peers yet) resolves
        // as "not found" rather than failing: a missing network must not
        // break loading the user's local data.
        let dht_bytes = match tokio::time::timeout(
            Self::RESOLVE_DHT_TIMEOUT,
            self.network.get_record(key.clone()),
        )
        .await
        {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(e)) => {
                tracing::warn!("DHT get_record failed: {e}");
                None
            }
            Err(_) => {
                tracing::warn!("DHT get_record timed out after {:?}", Self::RESOLVE_DHT_TIMEOUT);
                None
            }
        };
        let from_dht = dht_bytes.and_then(|bytes| {
            let record: AppPointerRecord = bincode::deserialize(&bytes).ok()?;
            (record.owner == *owner && record.name == name && record.verify()).then_some(record)
        });
        if let Some(record) = &from_dht {
            if let Ok(bytes) = bincode::serialize(record) {
                let _ = tokio::fs::create_dir_all(&self.config.records_path()).await;
                let _ = tokio::fs::write(&cache_path, bytes).await;
            }
        }
        Ok(from_dht)
    }

    /// Loads the user's latest `Profile` from the shared store, via the
    /// `(owner, "profile")` record. Returns `None` until one is published.
    pub async fn load_profile(&self) -> anyhow::Result<Option<Profile>> {
        let record = match self.resolve_pointer(self.identity.id(), RECORD_PROFILE).await? {
            Some(r) => r,
            None => return Ok(None),
        };
        let object = self.storage.get_verified(&record.manifest).await?;
        Ok(Some(object.decode()?))
    }

    /// Publishes a new `Profile` object and repoints `(owner, "profile")`.
    pub async fn save_profile(&self, profile: &Profile) -> anyhow::Result<ObjectId> {
        let version = self
            .load_profile()
            .await?
            .map(|p| p.version + 1)
            .unwrap_or(1);
        let mut profile = profile.clone();
        profile.version = version;
        let object = profile.to_object(&self.identity)?;
        let id = object.id.clone();
        self.storage.put_verified(&object).await?;
        // A profile is a public self-description: serve + announce it so
        // peers resolving `(owner, "profile")` (e.g. for friendly peer
        // display names) can fetch it.
        self.announce(id.clone()).await?;
        self.publish_pointer(RECORD_PROFILE, id.clone()).await?;
        Ok(id)
    }

    /// Claims a globally unique `username` for this identity: publishes the
    /// signed `(owner, "username")` record in the shared store and announces
    /// the `username:<name>` registry record on the DHT so others can
    /// reverse-resolve the name back to this identity.
    ///
    /// Usernames are lowercased and trimmed, and must be non-empty.
    pub async fn claim_username(&self, username: &str) -> anyhow::Result<()> {
        let username = normalize_username(username)?;
        let version = self
            .resolve_username(self.identity.id())
            .await?
            .map(|u| u.version + 1)
            .unwrap_or(1);
        let record = UsernameRecord {
            username: username.clone(),
            version,
        };
        let object = record.to_object(&self.identity)?;
        let id = object.id.clone();
        self.storage.put_verified(&object).await?;
        // A username claim is a public act by definition: serve the record
        // object so any peer that resolves the pointer can fetch it, and
        // announce ourselves as a DHT provider so `find_providers` finds us.
        self.announce(id.clone()).await?;
        self.publish_pointer(RECORD_USERNAME, id.clone()).await?;
        // Registry record: `username:<name>` → owner. Best-effort DHT put
        // (mirrors `publish_pointer`); a failure leaves the pointer readable
        // by those who know the owner, just not reverse-resolvable by name.
        let registry_key = format!("{USERNAME_REGISTRY_PREFIX}{username}").into_bytes();
        let value = self.identity.id().to_string().into_bytes();
        tokio::spawn({
            let network = self.network.clone();
            async move {
                if let Err(e) =
                    network.put_record(registry_key, value).await
                {
                    tracing::warn!("username registry put_record failed: {e}");
                }
            }
        });
        Ok(())
    }

    /// Resolves this identity's (or any owner's) claimed username from the
    /// signed `(owner, "username")` record.
    pub async fn resolve_username(
        &self,
        owner: &IdentityId,
    ) -> anyhow::Result<Option<UsernameRecord>> {
        let object = self.resolve_owner_object(owner, RECORD_USERNAME).await?;
        let Some(object) = object else { return Ok(None) };
        Ok(Some(object.decode()?))
    }

    /// Resolves `owner`'s signed `Profile` (display name, DH key, avatar)
    /// via the `(owner, "profile")` record, fetching from the network when
    /// it isn't cached locally.
    pub async fn resolve_profile(
        &self,
        owner: &IdentityId,
    ) -> anyhow::Result<Option<Profile>> {
        let object = self.resolve_owner_object(owner, RECORD_PROFILE).await?;
        let Some(object) = object else { return Ok(None) };
        Ok(Some(object.decode()?))
    }

    /// Resolves the signed, verified object a peer publishes under
    /// `(owner, name)` (e.g. `profile` or `username`), fetching it from the
    /// network when it isn't cached locally. Returns `None` when there is no
    /// record or the object doesn't verify against `owner`.
    async fn resolve_owner_object(
        &self,
        owner: &IdentityId,
        name: &str,
    ) -> anyhow::Result<Option<Object>> {
        let Some(record) = self.resolve_pointer(owner, name).await? else {
            return Ok(None);
        };
        match self.storage.get_verified(&record.manifest).await {
            Ok(object) if object.payload.owner == *owner => Ok(Some(object)),
            Ok(_) => Ok(None),
            Err(_) => {
                // The pointer may point at an object we haven't cached; try
                // to fetch it from the network so a *remote* profile/username
                // is resolvable. Best-effort: an unreachable owner resolves
                // to `None`. Hint the fetch at the owner's peer id (embedded
                // in their identity string) so it works even when the owner
                // never announced a DHT provider record.
                let hint = owner_peer_id(owner);
                match self.fetch_object(record.manifest.clone(), hint).await {
                    Ok(object) if object.payload.owner == *owner => Ok(Some(object)),
                    _ => Ok(None),
                }
            }
        }
    }

    /// Best-effort enrichment of connected peers: for each connected peer,
    /// resolve the identity it carries (via the `device:<peer-id>` registry,
    /// falling back to the pre-device-key assumption that the device id *is*
    /// the identity's embedded peer id) and then its signed username/profile
    /// display name, pushing the result back into the network manager's peer
    /// map so `peers()` and the UI can show friendly names instead of raw
    /// peer ids.
    ///
    /// Resolution is opportunistic: peers whose records can't be reached
    /// (offline DHT, not connected to bootstrap peers) simply keep `None` for
    /// the unknown fields. All peers are resolved *concurrently*, each with
    /// its own timeout, so one peer with no records (e.g. a bootstrap relay)
    /// can't starve the others or discard their results.
    pub async fn enrich_peers(&self) -> anyhow::Result<()> {
        const ENRICH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

        let peers = self.network.peers().await?;
        let resolutions = peers.into_iter().map(|peer| {
            let peer_id = peer.peer_id;
            // Per-peer timeout (not one shared deadline): a peer with no
            // records (e.g. a bootstrap relay) times out on its own without
            // discarding results already resolved for everyone else.
            async move {
                // Identity via the device registry; `resolved` is `false`
                // when we had to assume the legacy identity-bound peer id.
                let (identity, resolved) = match tokio::time::timeout(
                    ENRICH_TIMEOUT,
                    self.resolve_device_identity(&peer_id),
                )
                .await
                {
                    Ok(Ok(Some(id))) => (Some(id), true),
                    _ => (
                        Some(IdentityId::new(format!("canopee://identity/{peer_id}"))),
                        false,
                    ),
                };
                // Each record gets its OWN timeout inside the join: a peer
                // with a username but no profile must not lose its resolved
                // username just because the profile DHT lookup is slow to
                // return not-found.
                let (username, display_name) = tokio::join!(
                    async {
                        match &identity {
                            Some(id) => {
                                tokio::time::timeout(ENRICH_TIMEOUT, self.resolve_username(id))
                                    .await
                                    .ok()
                                    .and_then(|r| r.ok())
                                    .flatten()
                                    .map(|u| u.username)
                            }
                            None => None,
                        }
                    },
                    async {
                        match &identity {
                            Some(id) => {
                                tokio::time::timeout(ENRICH_TIMEOUT, self.resolve_profile(id))
                                    .await
                                    .ok()
                                    .and_then(|r| r.ok())
                                    .flatten()
                                    .map(|p| p.display_name)
                            }
                            None => None,
                        }
                    }
                );
                (peer_id, identity, username, display_name, resolved)
            }
        });
        let results = futures::future::join_all(resolutions).await;
        for (peer_id, identity, username, display_name, resolved) in results {
            if username.is_none() && display_name.is_none() && !resolved {
                continue;
            }
            let _ = self
                .network
                .set_peer_meta(peer_id, identity, username, display_name)
                .await;
        }
        Ok(())
    }

    /// Reverse-resolves a friendly `username` to its canonical owner
    /// (`canopee://identity/<peer-id>`) via the DHT registry, then verifies
    /// the claim by checking the returned owner really publishes a matching
    /// `(owner, "username")` record. `None` if the name is unclaimed or the
    /// registry record does not verify.
    pub async fn resolve_owner_from_username(
        &self,
        username: &str,
    ) -> anyhow::Result<Option<IdentityId>> {
        let username = normalize_username(username)?;
        let registry_key = format!("{USERNAME_REGISTRY_PREFIX}{username}").into_bytes();
        let Some(bytes) = self
            .network
            .get_record(registry_key)
            .await
            .map_err(|e| anyhow::anyhow!("username lookup failed: {e}"))?
        else {
            return Ok(None);
        };
        let id_string = String::from_utf8(bytes)
            .map_err(|_| anyhow::anyhow!("username registry record is not valid utf-8"))?;
        let owner = IdentityId::new(id_string);
        // Verify the claim: the owner must publish a (owner,"username")
        // record that matches the requested name.
        match self.resolve_username(&owner).await? {
            Some(record) if record.username == username => Ok(Some(owner)),
            Some(_) => {
                tracing::warn!("username {username} claimed by an owner publishing a different name");
                Ok(None)
            }
            None => Ok(None),
        }
    }

    // ---- devices ("one identity, many machines") ----

    /// The DHT record key mapping a device's network `PeerId` back to the
    /// identity it carries: `device:<peer-id>` → `canopee://identity/<id>`.
    fn device_registry_key(device_id: &PeerId) -> Vec<u8> {
        format!("{DEVICE_REGISTRY_PREFIX}{device_id}").into_bytes()
    }

    /// Loads the list of devices carrying an identity, via the
    /// `(owner, "devices")` record (fetching from the network when not
    /// cached). `None` until the owner has registered at least one device.
    pub async fn load_device_list(
        &self,
        owner: &IdentityId,
    ) -> anyhow::Result<Option<DeviceList>> {
        let object = self.resolve_owner_object(owner, RECORD_DEVICES).await?;
        let Some(object) = object else { return Ok(None) };
        Ok(Some(object.decode()?))
    }

    /// Publishes a new `DeviceList` snapshot and repoints
    /// `(owner, "devices")`.
    ///
    /// A device list is a public register (that's the point — other devices
    /// of the identity must be able to read it), so the object is announced:
    /// it is served and advertised as a DHT provider, exactly like profiles
    /// and usernames.
    pub async fn save_device_list(&self, list: &DeviceList) -> anyhow::Result<ObjectId> {
        let version = self
            .load_device_list(self.identity.id())
            .await?
            .map(|l| l.version + 1)
            .unwrap_or(1);
        let mut list = list.clone();
        list.version = version;
        let object = list.to_object(&self.identity)?;
        let id = object.id.clone();
        self.storage.put_verified(&object).await?;
        self.announce(id.clone()).await?;
        self.publish_pointer(RECORD_DEVICES, id.clone()).await?;
        Ok(id)
    }

    /// Adds (or refreshes) one device entry on the owner's device list and
    /// republishes it. Re-registering an existing device keeps its original
    /// `added_at`.
    pub async fn add_device(
        &self,
        device_id: &str,
        device_name: &str,
    ) -> anyhow::Result<DeviceList> {
        let mut list = self.load_device_list(self.identity.id()).await?.unwrap_or(DeviceList {
            devices: vec![],
            version: 0,
        });
        let added_at = list.by_device_id(device_id).and_then(|d| d.added_at);
        if !list.devices.iter().any(|d| d.device_id == device_id) {
            list.devices.push(DeviceEntry {
                device_id: device_id.to_string(),
                device_name: device_name.to_string(),
                added_at: added_at.or(Some(OffsetDateTime::now_utc())),
            });
        } else if let Some(entry) = list.devices.iter_mut().find(|d| d.device_id == device_id) {
            entry.device_name = device_name.to_string();
            entry.added_at = added_at.or(Some(OffsetDateTime::now_utc()));
        }
        let _ = self.save_device_list(&list).await?;
        Ok(list)
    }

    /// Removes one device from the owner's device list and republishes it.
    pub async fn remove_device(&self, device_id: &str) -> anyhow::Result<DeviceList> {
        let mut list = self
            .load_device_list(self.identity.id())
            .await?
            .unwrap_or(DeviceList {
                devices: vec![],
                version: 0,
            });
        list.devices.retain(|d| d.device_id != device_id);
        let _ = self.save_device_list(&list).await?;
        Ok(list)
    }

    /// Registers this device with its identity: upserts the device into the
    /// `(owner, "devices")` list and publishes the `device:<peer-id>` →
    /// identity registry record so other peers can reverse-resolve the
    /// machine. Best-effort by design — a DHT failure must not fail startup;
    /// the durable local pieces (object, pointer, registry cache) are what
    /// make the device resolvable in the common same-machine + LAN cases.
    async fn register_device(&self) {
        let peer_id = self.device_key.peer_id();
        if let Err(e) = self
            .add_device(&peer_id.to_string(), self.device_key.device_name())
            .await
        {
            tracing::warn!("registering this device in the device list failed: {e}");
        }
        self.publish_device_registry().await;
    }

    /// Schedules one DHT re-publication of this device's registry record,
    /// device-list pointer, and device-list provider announcement after the
    /// network becomes reachable. At startup these records are pushed while
    // the swarm has no peers, so `put_record` / `announce` silently fail;
    // this background task catches them up so remote peers can resolve the
    /// identity → device mapping within seconds of the first connection.
    fn schedule_device_publish(&self) {
        use canopee_storage::AppPointerRecord as Apr;

        let network = self.network.clone();
        let identity_id = self.identity.id().clone();
        let records_path = self.config.records_path();
        let registry_key = Self::device_registry_key(&self.device_key.peer_id());
        let registry_value = identity_id.to_string().into_bytes();

        tokio::spawn(async move {
            // Wait until the swarm is connected to at least one peer (the
            // bootstrap relay in practice), then catch up. Give up after
            // ~25s so a completely offline start doesn't hang a spawned task
            // forever — the records stay cached locally and will be picked up
            // when the node goes online next time.
            for _ in 0..50 {
                if let Ok(peers) = network.peers().await {
                    if !peers.is_empty() {
                        break;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }

            // 1. device registry: `device:<peer-id>` → identity string.
            if let Err(e) = network
                .put_record(registry_key, registry_value)
                .await
            {
                tracing::warn!("device registry re-publication failed: {e}");
            }

            // 2. `(owner, "devices")` pointer + provider announcement so
            //    remote peers can `find_providers` the device-list object.
            let key = Apr::key(&identity_id, RECORD_DEVICES);
            let path = records_path.join(format!("{}.record", hex::encode(&key)));
            if let Ok(bytes) = tokio::fs::read(&path).await {
                if let Ok(record) = bincode::deserialize::<Apr>(&bytes) {
                    if record.verify() && record.owner == identity_id {
                        let _ = network.put_record(key, bytes).await;
                        let _ = network.announce(record.manifest.clone()).await;
                    }
                }
            }
        });
    }

    /// Publishes `device:<own-peer-id>` → `canopee://identity/<own-id>` both
    /// durably on this machine (the local record cache) and best-effort on
    /// the DHT.
    async fn publish_device_registry(&self) {
        let key = Self::device_registry_key(&self.device_key.peer_id());
        let value = self.identity.id().to_string().into_bytes();
        self.publish_record(key, value).await;
    }

    /// Reverse-resolves a device's network `PeerId` back to the identity it
    /// carries, from the `device:<peer-id>` registry (local cache first,
    /// then DHT). `None` for devices that never registered (e.g. legacy
    /// pre-device-key peers or relays).
    pub async fn resolve_device_identity(
        &self,
        device_id: &PeerId,
    ) -> anyhow::Result<Option<IdentityId>> {
        let key = Self::device_registry_key(device_id);
        let Some(bytes) = self.resolve_record(&key).await? else {
            return Ok(None);
        };
        let id_string = String::from_utf8(bytes)
            .map_err(|_| anyhow::anyhow!("device registry record is not valid utf-8"))?;
        Ok(Some(IdentityId::new(id_string)))
    }

    /// Resolves which device of `owner` to dial, via the `(owner, "devices")`
    /// list. Returns this machine's own device when `owner` is the local
    /// identity, or the first well-formed device id on the owner's list.
    pub async fn resolve_device_peer_id(
        &self,
        owner: &IdentityId,
    ) -> anyhow::Result<Option<PeerId>> {
        if owner == self.identity.id() {
            return Ok(Some(self.device_key.peer_id()));
        }
        let Some(list) = self.load_device_list(owner).await? else {
            return Ok(None);
        };
        Ok(list
            .devices
            .iter()
            .find_map(|d| d.device_id.parse().ok()))
    }

    /// Writes `key → value` durably to the local record cache (awaited — it
    /// is the authoritative same-machine view) and best-effort to the DHT
    /// (fire-and-forget, zero-downtime by design).
    async fn publish_record(&self, key: Vec<u8>, value: Vec<u8>) {
        if let Err(e) = self.cache_record(&key, &value).await {
            tracing::warn!("caching record {key:?} failed: {e}");
        }
        let network = self.network.clone();
        tokio::spawn(async move {
            if let Err(e) = network.put_record(key, value).await {
                tracing::warn!("DHT put_record failed (record cached locally): {e}");
            }
        });
    }

    /// Resolves `key` from the local record cache first, then the DHT
    /// (bounded — see `RESOLVE_DHT_TIMEOUT`). Cache hits are cached back on
    /// DHT success.
    async fn resolve_record(&self, key: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
        let cache_path = self
            .config
            .records_path()
            .join(format!("{}.record", hex::encode(key)));
        if let Ok(bytes) = tokio::fs::read(&cache_path).await {
            return Ok(Some(bytes));
        }
        let dht = match tokio::time::timeout(
            Self::RESOLVE_DHT_TIMEOUT,
            self.network.get_record(key.to_vec()),
        )
        .await
        {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(e)) => {
                tracing::warn!("DHT get_record failed: {e}");
                None
            }
            Err(_) => {
                tracing::warn!("DHT get_record timed out after {:?}", Self::RESOLVE_DHT_TIMEOUT);
                None
            }
        };
        if let Some(value) = &dht {
            let _ = self.cache_record(key, value).await;
        }
        Ok(dht)
    }

    async fn cache_record(&self, key: &[u8], value: &[u8]) -> anyhow::Result<()> {
        let cache_path = self.config.records_path();
        tokio::fs::create_dir_all(&cache_path).await?;
        let path = cache_path.join(format!("{}.record", hex::encode(key)));
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, value).await?;
        tokio::fs::rename(&tmp, path).await?;
        Ok(())
    }

    /// Loads the user's latest `ContactList` from the shared store, via the
    /// `(owner, "contacts")` record.
    pub async fn load_contact_list(&self) -> anyhow::Result<Option<ContactList>> {
        let record = match self.resolve_pointer(self.identity.id(), RECORD_CONTACTS).await? {
            Some(r) => r,
            None => return Ok(None),
        };
        let object = self.storage.get_verified(&record.manifest).await?;
        Ok(Some(object.decode()?))
    }

    /// Publishes a new `ContactList` snapshot and repoints
    /// `(owner, "contacts")`.
    pub async fn save_contact_list(&self, list: &ContactList) -> anyhow::Result<ObjectId> {
        let version = self
            .load_contact_list()
            .await?
            .map(|l| l.version + 1)
            .unwrap_or(1);
        let mut list = list.clone();
        list.version = version;
        let object = list.to_object(&self.identity)?;
        let id = object.id.clone();
        self.storage.put_verified(&object).await?;
        self.publish_pointer(RECORD_CONTACTS, id.clone()).await?;
        Ok(id)
    }

    /// Loads the user's latest `HomeIndex` from the shared store, via the
    /// `(owner, "home")` record.
    pub async fn load_home_index(&self) -> anyhow::Result<Option<HomeIndex>> {
        let record = match self.resolve_pointer(self.identity.id(), RECORD_HOME).await? {
            Some(r) => r,
            None => return Ok(None),
        };
        let object = self.storage.get_verified(&record.manifest).await?;
        Ok(Some(object.decode()?))
    }

    /// Publishes a new `HomeIndex` and repoints `(owner, "home")`.
    ///
    /// Saving an index also reconciles network visibility: entries that newly
    /// became `shared: true` are marked shared and announced as providers;
    /// entries that flipped back to `shared: false` are withdrawn. Both
    /// network operations are best-effort.
    pub async fn save_home_index(&self, index: &HomeIndex) -> anyhow::Result<ObjectId> {
        let previous = self.load_home_index().await?;
        let version = previous.as_ref().map(|i| i.version + 1).unwrap_or(1);
        let mut index = index.clone();
        index.version = version;
        let object = index.to_object(&self.identity)?;
        let id = object.id.clone();
        self.storage.put_verified(&object).await?;
        self.publish_pointer(RECORD_HOME, id.clone()).await?;
        self.reconcile_shared(previous.as_ref(), Some(&index)).await;
        Ok(id)
    }

    /// Flips one home entry's `shared` flag and republishes the index —
    /// the explicit "share this on the network" / "stop sharing" action for
    /// a single file or picture. Returns the new index object id.
    pub async fn set_home_entry_shared(
        &self,
        name: &str,
        shared: bool,
    ) -> anyhow::Result<ObjectId> {
        let mut index = self
            .load_home_index()
            .await?
            .ok_or_else(|| anyhow::anyhow!("no home index yet — save one before sharing entries"))?;
        let entry = index
            .entries
            .iter_mut()
            .find(|e| e.name == name)
            .ok_or_else(|| anyhow::anyhow!("no home entry named {name:?}"))?;
        let object = entry.object.clone();
        entry.shared = shared;
        let index_id = self.save_home_index(&index).await?;
        if shared {
            // A freshly-shared entry must be findable by name from other
            // machines: publish the `(owner, "entry:<name>")` pointer. Only
            // sharing publishes it — private entries never leak into records.
            self.publish_entry_pointer(name, &object).await;
        }
        Ok(index_id)
    }

    /// Shares a stored object under `name`: upserts a `shared: true` entry in
    /// the home index (creating the index on first use) and announces the
    /// object as a DHT provider. This is the one-call "put this file on the
    /// network" action. Returns the new index object id.
    pub async fn share_object(
        &self,
        name: &str,
        id: &ObjectId,
        app: Option<String>,
    ) -> anyhow::Result<ObjectId> {
        let object = self.storage.get_verified(id).await?;
        let object_type = object.object_type();
        let mut index = self.load_home_index().await?.unwrap_or(HomeIndex {
            profile: None,
            contacts: None,
            entries: vec![],
            version: 0,
        });
        match index.entries.iter_mut().find(|e| e.name == name) {
            Some(entry) => {
                entry.object = id.clone();
                entry.object_type = object_type;
                entry.shared = true;
                entry.app = app;
            }
            None => index.entries.push(canopee_storage::HomeEntry {
                name: name.to_string(),
                object: id.clone(),
                object_type,
                shared: true,
                app,
            }),
        }
        let index_id = self.save_home_index(&index).await?;
        // A share must be resolvable by name from other machines, so publish
        // the `(owner, "entry:<name>")` pointer alongside the index.
        self.publish_entry_pointer(name, id).await;
        Ok(index_id)
    }

    /// Publishes the `(owner, "entry:<name>")` pointer record pointing at
    /// `id`, so remote peers can resolve a shared entry by its human name.
    /// Best-effort: the record is durably cached locally and fire-and-forget
    /// on the DHT (see [`Self::publish_pointer`]).
    async fn publish_entry_pointer(&self, name: &str, id: &ObjectId) {
        if let Err(e) = self.publish_pointer(&format!("entry:{name}"), id.clone()).await {
            tracing::warn!("publishing entry pointer {name:?} failed: {e}");
        }
    }

    /// Diffs the `shared` flags between two home-index versions and applies
    /// the result to the serving set: newly shared objects are announced,
    /// newly unshared ones are withdrawn. The shared-set membership (which
    /// gates serving) is updated synchronously; the DHT announce/unannounce
    /// runs in the background so a slow network never stalls a local save.
    async fn reconcile_shared(&self, previous: Option<&HomeIndex>, current: Option<&HomeIndex>) {
        let shared_ids = |index: Option<&HomeIndex>| -> std::collections::HashSet<ObjectId> {
            index
                .map(|i| {
                    i.entries
                        .iter()
                        .filter(|e| e.shared)
                        .map(|e| e.object.clone())
                        .collect()
                })
                .unwrap_or_default()
        };
        let before = shared_ids(previous);
        let after = shared_ids(current);
        for id in after.difference(&before) {
            self.shared.insert(id).await;
            let network = self.network.clone();
            let id = id.clone();
            tokio::spawn(async move {
                if let Err(e) = network.announce(id.clone()).await {
                    tracing::warn!("announce of shared entry {id} failed: {e}");
                }
            });
        }
        for id in before.difference(&after) {
            self.shared.remove(id).await;
            let network = self.network.clone();
            let id = id.clone();
            tokio::spawn(async move {
                let _ = network.unannounce(id).await;
            });
        }
    }

    /// Fetches `object_id` from the network and imports it into the local
    /// store, verifying its signature along the way. Tries DHT providers
    /// first, then `from`. Returns the imported object.
    pub async fn fetch_object(
        &self,
        object_id: ObjectId,
        from: Option<PeerId>,
    ) -> anyhow::Result<Object> {
        if let Ok(object) = self.storage.get_verified(&object_id).await {
            return Ok(object);
        }
        let providers = self.network.find_providers(object_id.clone()).await?;
        for provider in providers {
            match self.network.get_object(provider, object_id.clone()).await {
                Ok(bundle) => {
                    let object = bundle.object.clone();
                    self.import(bundle).await?;
                    return Ok(object);
                }
                Err(e) => tracing::warn!("fetch from {provider} failed: {e}"),
            }
        }
        if let Some(peer) = from {
            let bundle = self.network.get_object(peer, object_id).await?;
            let object = bundle.object.clone();
            self.import(bundle).await?;
            return Ok(object);
        }
        Err(anyhow::anyhow!("no provider for object {object_id}"))
    }

    // ---- LAN device pairing ("share the identity onto another machine") ----

    /// Picks the address the pairing QR advertises for the other device to
    /// dial: the first non-loopback listen address (LAN IPv4 in practice),
    /// falling back to loopback so pairing works on single-NIC machines and
    /// in tests. Appends this device's `/p2p/<peer-id>` so the result is a
    /// directly dialable full multiaddr string.
    async fn dialable_lan_addr(&self) -> anyhow::Result<String> {
        let addrs = self.network.listen_addresses().await?;
        let is_ip = |s: &str| s.starts_with("/ip4/") || s.starts_with("/ip6/");
        let is_loopback = |s: &str| s.starts_with("/ip4/127.") || s.starts_with("/ip6/::1");
        let pick = addrs
            .iter()
            .map(|a| a.to_string())
            .find(|s| is_ip(s) && !is_loopback(s))
            .or_else(|| addrs.iter().map(|a| a.to_string()).find(|s| is_ip(s)))
            .ok_or_else(|| anyhow::anyhow!("no listen address available to pair over"))?;
        let base = pick.split("/p2p/").next().unwrap_or(&pick);
        if !is_ip(base) {
            anyhow::bail!("listen address {pick} is not a dialable ip address");
        }
        Ok(format!("{}/p2p/{}", base, self.device_key.peer_id()))
    }

    /// Starts a device-pairing session on THIS device (the new device): mints
    /// a fresh 12-char pairing code + one-time session id and returns the
    /// `PairingQrData` to display or print out of band. The node keeps the
    /// code in memory only, so it can decrypt the payload the source device
    /// sends back; calling again replaces the previous session.
    pub async fn initiate_pairing(&self) -> anyhow::Result<PairingQrData> {
        let code = canopee_identity::pairing::generate_code();
        let session_id = canopee_identity::pairing::generate_session_id();
        let lan_addr = self.dialable_lan_addr().await?;

        *self.pairing.write().await = Some(OwnPairing {
            session_id: session_id.clone(),
            code: code.clone(),
        });

        Ok(PairingQrData {
            version: 1,
            device_id: self.device_key.peer_id().to_string(),
            device_name: self.device_key.device_name().to_string(),
            lan_addr,
            code,
            session_id,
        })
    }

    /// The counterpart to [`Self::initiate_pairing`], run on the device that
    /// already carries the identity (the source). `qr` is what the new device
    /// displayed; `code` is what the user typed (or scanned) to approve —
    /// the explicit-approve step that makes pairing safe against a rogue
    /// device reaching the wire. Verifies the two match (constant-time),
    /// encrypts this device's identity + signed user records under the session
    /// key, dials the new device on the LAN and delivers the payload, awaiting
    /// its acceptance. Returns the new device's status message.
    pub async fn complete_pairing(
        &self,
        qr: PairingQrData,
        code: &str,
    ) -> anyhow::Result<String> {
        if qr.version != 1 {
            anyhow::bail!("unsupported pairing protocol version {}", qr.version);
        }
        if !canopee_identity::pairing::codes_match(&qr.code, code) {
            anyhow::bail!(
                "pairing code does not match the code on the device — check it and retry"
            );
        }
        let new_device: PeerId = qr
            .device_id
            .parse()
            .map_err(|e| anyhow::anyhow!("device id in the QR is not a valid peer id: {e}"))?;
        if new_device == self.device_key.peer_id() {
            anyhow::bail!("this device is already the one carrying the identity — nothing to pair");
        }

        let session_key = self.pairing_session_key(&qr, new_device, code)?;
        let payload = self.encrypt_pairing_payload(&session_key).await?;

        let request = CanopeePairingRequest {
            from: self.device_key.peer_id().to_string(),
            device_id: qr.device_id.clone(),
            session_id: qr.session_id.clone(),
            payload,
        };
        self.send_pairing_request(new_device, &qr.lan_addr, request)
            .await
    }

    /// Derives the 256-bit pairing session key from the code + one-time
    /// session id, bound to both device ids. Both sides build this salt
    /// identically (source: `session_id || source || new`; new device:
    /// `session_id || from || self`), so only the device that ever saw the
    /// code can reproduce the key — the payload is useless to a passive
    /// eavesdropper even if it observes the whole exchange.
    fn pairing_session_key(&self, qr: &PairingQrData, new_device: PeerId, code: &str) -> anyhow::Result<[u8; 32]> {
        let mut salt = Vec::from(b"canopee/pairing/v1");
        salt.extend_from_slice(qr.session_id.as_bytes());
        salt.extend_from_slice(self.device_key.peer_id().to_bytes().as_slice());
        salt.extend_from_slice(new_device.to_bytes().as_slice());
        Ok(canopee_identity::pairing::derive_session_key(code, &salt))
    }

    /// Builds + encrypts the pairing payload sent to the new device: this
    /// device's raw identity keypair bytes plus its signed user records
    /// (device list, profile, contacts) with their `(owner, name)` pointers.
    /// Records are read from the local record cache + object store, so no DHT
    /// round-trips are involved; anything missing locally is simply skipped.
    async fn encrypt_pairing_payload(&self, session_key: &[u8; 32]) -> anyhow::Result<PairingPayload> {
        let identity_key = self.identity.export_bytes()?;

        let mut records = Vec::new();
        for name in [RECORD_DEVICES, RECORD_PROFILE, RECORD_CONTACTS] {
            let key = AppPointerRecord::key(self.identity.id(), name);
            let path = self
                .config
                .records_path()
                .join(format!("{}.record", hex::encode(&key)));
            let Ok(bytes) = tokio::fs::read(&path).await else {
                continue;
            };
            let Ok(pointer) = bincode::deserialize::<AppPointerRecord>(&bytes) else {
                continue;
            };
            if pointer.owner != *self.identity.id() || !pointer.verify() {
                continue;
            }
            let Ok(object) = self.storage.get_verified(&pointer.manifest).await else {
                continue;
            };
            if !object.verify() || object.payload.owner != *self.identity.id() {
                continue;
            }
            records.push(PairingRecord {
                name: name.to_string(),
                object,
                pointer,
            });
        }

        let data = PairingData {
            version: 1,
            identity_key,
            records,
        };
        let encrypted = canopee_identity::pairing::encrypt_payload(
            &bincode::serialize(&data)?,
            session_key,
        )?;
        Ok(PairingPayload { encrypted })
    }

    /// Dials the new device's advertised LAN address and delivers the pairing
    /// request over `/canopee/pairing/1.0.0`, waiting until the device is
    /// actually reachable before sending (request-response can only route
    /// once the outbound connection exists). Errors surface the new device's
    /// refusal or the transport failure to the user.
    async fn send_pairing_request(
        &self,
        device: PeerId,
        lan_addr: &str,
        request: CanopeePairingRequest,
    ) -> anyhow::Result<String> {
        const DIAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

        let addr: Multiaddr = lan_addr.parse()?;
        self.network.dial(addr).await?;

        for _ in 0..40 {
            if let Ok(peers) = self.network.peers().await {
                if peers.iter().any(|p| p.peer_id == device) {
                    return self.network.send_pairing(device, request).await;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        anyhow::bail!("the device at {lan_addr} did not come online within {DIAL_TIMEOUT:?}")
    }

    /// Handles one inbound `/canopee/pairing/1.0.0` request on the *new*
    /// device. Every check is strict: the payload must target this device's
    /// pending session, the `from` field must match the actual sender peer,
    /// the AEAD must decrypt under the code-derived key, and every transferred
    /// record must verify against the transferred identity. Only then is the
    /// identity written to this device's key file (honoring the at-rest
    /// policy) and the records imported + cached locally. The live runtime
    /// keeps running on its current (temporary) identity — the imported one
    /// takes effect on the next restart.
    async fn accept_pairing(&self, incoming: InboundPairing) -> CanopeePairingResponse {
        // Pairing sessions are single-use: take the session out of memory
        // whether or not this request turns out to be valid.
        let session = self.pairing.write().await.take();
        let Some(session) = session.filter(|s| s.session_id == incoming.request.session_id) else {
            tracing::warn!(
                "ignored pairing request {}: no matching session on this device",
                incoming.request.session_id
            );
            return CanopeePairingResponse::Error("no pairing session for this request".into());
        };

        // The request must vouch for the same two devices that derived the
        // session key: `from` = the actual sender peer, `device_id` = us.
        let from: PeerId = match incoming.request.from.parse() {
            Ok(peer) if peer == incoming.peer => peer,
            _ => {
                tracing::warn!("ignored pairing request: `from` does not match the sender");
                return CanopeePairingResponse::Error("sender identity mismatch".into());
            }
        };
        let target: PeerId = match incoming.request.device_id.parse() {
            Ok(peer) if peer == self.device_key.peer_id() => peer,
            _ => {
                tracing::warn!("ignored pairing request: not addressed to this device");
                return CanopeePairingResponse::Error("wrong destination device".into());
            }
        };

        // Session key from the held code; the AEAD then vouches for the code —
        // a wrong or absent code fails decryption here, tampering included.
        let mut salt = Vec::from(b"canopee/pairing/v1");
        salt.extend_from_slice(incoming.request.session_id.as_bytes());
        salt.extend_from_slice(from.to_bytes().as_slice());
        salt.extend_from_slice(target.to_bytes().as_slice());
        let key = canopee_identity::pairing::derive_session_key(&session.code, &salt);
        let clear = match canopee_identity::pairing::decrypt_payload(
            &incoming.request.payload.encrypted,
            &key,
        ) {
            Ok(clear) => clear,
            Err(e) => {
                tracing::warn!("pairing request rejected: {e}");
                return CanopeePairingResponse::Error(e.to_string());
            }
        };
        let data: PairingData = match bincode::deserialize::<PairingData>(&clear) {
            Ok(data) if data.version == 1 => data,
            _ => {
                tracing::warn!("pairing request rejected: malformed payload");
                return CanopeePairingResponse::Error("malformed pairing payload".into());
            }
        };
        let imported = match Identity::import_bytes(&data.identity_key) {
            Ok(identity) => identity,
            Err(e) => return CanopeePairingResponse::Error(e.to_string()),
        };

        // Every record must be signed by (and address) the transferred
        // identity, and the pointer must resolve to the very object carried.
        for record in &data.records {
            let valid = record.pointer.owner == *imported.id()
                && record.pointer.manifest == record.object.id
                && record.pointer.name == record.name
                && record.pointer.verify()
                && record.object.verify()
                && record.object.payload.owner == *imported.id();
            if !valid {
                let reason = "the transferred records do not verify against the transferred identity";
                tracing::warn!("pairing request rejected: {reason}");
                return CanopeePairingResponse::Error(reason.into());
            }
        }

        if let Err(e) = self.set_identity_key(&imported).await {
            tracing::warn!("pairing request rejected: {e}");
            return CanopeePairingResponse::Error(e.to_string());
        }
        for record in &data.records {
            if let Err(e) = self.storage.import(&record.object).await {
                tracing::warn!("pairing import of {:?} failed: {e}", record.object.id);
                return CanopeePairingResponse::Error(format!(
                    "record {} could not be stored: {e}",
                    record.name
                ));
            }
            self.cache.mark_cached(&record.object.id).await;
            let key = AppPointerRecord::key(imported.id(), &record.name);
            if let Ok(bytes) = bincode::serialize(&record.pointer) {
                if let Err(e) = self.cache_record(&key, &bytes).await {
                    tracing::warn!("pairing record cache write failed: {e}");
                }
            }
        }

        let message = format!(
            "accepted — restart {} ({} ) to take over the identity",
            self.device_key.device_name(),
            imported.id()
        );
        tracing::info!("{message}");
        CanopeePairingResponse::Accepted(message)
    }

    /// Writes `imported` to this device's identity key file, honoring the
    /// same at-rest policy as `open_with_config` (`CANOPEE_IDENTITY_PASS` →
    /// encrypted, otherwise plaintext). The previous key is never deleted —
    /// it is backed up to `identity.key.bak-<timestamp>` first.
    async fn set_identity_key(&self, imported: &Identity) -> anyhow::Result<()> {
        let identity_path = self.config.identity_path().join("identity.key");
        let exists = tokio::fs::try_exists(&identity_path).await.unwrap_or(false);
        if exists {
            let backup = self
                .config
                .identity_path()
                .join(format!(
                    "identity.key.bak-{}",
                    OffsetDateTime::now_utc().unix_timestamp()
                ));
            tokio::fs::copy(&identity_path, &backup).await?;
        }
        let passphrase: Option<String> = std::env::var_os("CANOPEE_IDENTITY_PASS")
            .and_then(|p| p.into_string().ok());
        let bytes = match passphrase.as_deref() {
            Some(pass) => imported.export_encrypted(pass)?,
            None => imported.export_bytes()?,
        };
        let tmp = self.config.identity_path().join("identity.key.tmp");
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(&tmp, &identity_path).await?;
        Ok(())
    }

    /// Spawns the background task answering inbound `/canopee/pairing/1.0.0`
    /// requests for the lifetime of this runtime. Each request is handled
    /// strictly (session → decrypt → verify → persist); failures are answered
    /// with `CanopeePairingResponse::Error`, which the source device surfaces
    /// to the person running `canopee pair`.
    fn spawn_pairing_handler(&self) {
        let runtime = self.clone();
        tokio::spawn(async move {
            let mut events = runtime.network.pairing_events();
            while let Ok(incoming) = events.recv().await {
                let reply = incoming.reply.clone();
                let response = runtime.accept_pairing(incoming).await;
                if reply.send(response).is_err() {
                    tracing::warn!("pairing reply channel closed before answering");
                }
            }
        });
    }

    /// How often the background sync task refreshes user records from the
    /// DHT. 30s is aggressive enough for interactive use; a battery-conscious
    /// device would want this longer (a future config knob).
    const PERIODIC_SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

    /// Background task that periodically refreshes the identity-scoped user
    /// records (profile, contacts, devices) from the DHT. Only runs when the
    /// node is online (`started`) and has at least one connected peer — an
    /// isolated node has nothing to sync from.
    fn spawn_periodic_sync(&self) {
        let runtime = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Self::PERIODIC_SYNC_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let started = runtime.state.read().await.started;
                if !started {
                    continue;
                }
                let peer_count = runtime
                    .network
                    .peers()
                    .await
                    .map(|p| p.len())
                    .unwrap_or(0);
                if peer_count == 0 {
                    continue;
                }
                match runtime.sync_with_all_devices().await {
                    Ok(result) if result.any_updated() => {
                        tracing::info!("periodic sync: {result:?}");
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("periodic sync failed: {e}"),
                }
            }
        });
    }

    // ---- sync ("keep paired devices in step") ----

    /// Refreshes this node's user records (profile, contacts, devices) from
    /// the network. `peer_id` is accepted for API compatibility with the
    /// plan's per-device sync flow; the actual refresh is identity-scoped
    /// (all devices share the same DHT keys), so it runs once regardless of
    /// which peer is named.
    pub async fn sync_with_peer(&self, peer_id: PeerId) -> anyhow::Result<SyncResult> {
        tracing::info!("syncing records (peer hint: {peer_id})");
        self.sync_records().await
    }

    /// Refreshes this node's user records from every device in its
    /// `(owner, "devices")` list. The record refresh itself is
    /// identity-scoped, so it runs once after loading the device list.
    pub async fn sync_with_all_devices(&self) -> anyhow::Result<SyncResult> {
        let devices = self.load_device_list(self.identity.id()).await?;
        tracing::info!(
            "syncing records across {} registered device(s)",
            devices.map(|d| d.devices.len()).unwrap_or(0)
        );
        self.sync_records().await
    }

    /// Refreshes the three user records (profile, contacts, devices) from
    /// the DHT, last-writer-wins by the signed pointer's `published_at`.
    async fn sync_records(&self) -> anyhow::Result<SyncResult> {
        let mut result = SyncResult::default();
        for name in [RECORD_PROFILE, RECORD_CONTACTS, RECORD_DEVICES] {
            if self.sync_record(name).await? {
                match name {
                    RECORD_PROFILE => result.profile_updated = true,
                    RECORD_CONTACTS => result.contacts_updated = true,
                    RECORD_DEVICES => result.devices_updated = true,
                    _ => {}
                }
            }
        }
        Ok(result)
    }

    /// Refreshes one user record from the DHT: fetches the latest signed
    /// pointer (bypassing the local cache), compares `published_at` against
    /// the cached pointer, and if the DHT's is newer, fetches the referenced
    /// object into local storage and updates the cache. Returns `true` when
    /// the record was refreshed.
    async fn sync_record(&self, name: &str) -> anyhow::Result<bool> {
        let owner = self.identity.id().clone();
        let key = AppPointerRecord::key(&owner, name);
        // Fresh DHT fetch (bypass the local cache fast-path).
        let dht_bytes = match tokio::time::timeout(
            Self::RESOLVE_DHT_TIMEOUT,
            self.network.get_record(key.clone()),
        )
        .await
        {
            Ok(Ok(Some(bytes))) => bytes,
            Ok(Ok(None)) => return Ok(false), // no DHT record → nothing to sync
            Ok(Err(e)) => {
                tracing::warn!("sync: DHT get_record for {name} failed: {e}");
                return Ok(false);
            }
            Err(_) => {
                tracing::warn!("sync: DHT get_record for {name} timed out");
                return Ok(false);
            }
        };
        let dht_record: AppPointerRecord = match bincode::deserialize::<AppPointerRecord>(&dht_bytes) {
            Ok(r) if r.owner == owner && r.name == name && r.verify() => r,
            _ => return Ok(false),
        };
        // Compare with the local cache.
        let cache_path = self
            .config
            .records_path()
            .join(format!("{}.record", hex::encode(&key)));
        let local_record: Option<AppPointerRecord> = tokio::fs::read(&cache_path)
            .await
            .ok()
            .and_then(|bytes| bincode::deserialize(&bytes).ok())
            .filter(|r: &AppPointerRecord| {
                r.owner == owner && r.name == name && r.verify()
            });
        let is_newer = match &local_record {
            Some(local) => dht_record.published_at > local.published_at,
            None => true,
        };
        if !is_newer {
            return Ok(false);
        }
        // Fetch the referenced object (checks local storage first, then DHT
        // providers) and update the local cache.
        self.fetch_object(dht_record.manifest.clone(), None).await?;
        let bytes = bincode::serialize(&dht_record)?;
        tokio::fs::create_dir_all(&self.config.records_path()).await?;
        let tmp = cache_path.with_extension("tmp");
        tokio::fs::write(&tmp, &bytes).await?;
        tokio::fs::rename(&tmp, &cache_path).await?;
        tracing::info!("sync: refreshed {name} (published_at {})", dht_record.published_at);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canopee_identity::Identity;
    use canopee_storage::ObjectType;
    use std::sync::Once;

    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static SETUP: Once = Once::new();

    /// Points `HOME` at a scratch dir so `Runtime::open` and every peer it
    /// spawns stay far away from the developer's real `~/.canopee`. Tests
    /// that touch `Runtime` must hold `LOCK` (the env mutation is process
    /// global), and share one `TEST_HOME` to only set the env var once.
    fn with_scratch_home() -> &'static PathBuf {
        SETUP.call_once(|| {
            let dir = std::env::temp_dir().join(format!(
                "canopee_runtime_test_{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            // Leak so `std::env::set_var` outlives the test.
            let dir: &'static PathBuf = Box::leak(Box::new(dir));
            unsafe {
                std::env::set_var("HOME", dir);
            }
        });
        static HOME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        HOME.get_or_init(|| {
            std::env::temp_dir().join(format!("canopee_runtime_test_{}", std::process::id()))
        })
    }

    #[tokio::test]
    async fn import_marks_non_owned_object_as_cached() {
        let _guard = LOCK.lock().unwrap();
        let _ = with_scratch_home();
        let runtime = Runtime::open().await.unwrap();

        // An object created by a *different* identity, imported into this
        // runtime.
        let other_dir = std::env::temp_dir().join(format!(
            "canopee_runtime_test_other_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&other_dir).unwrap();
        let other = Arc::new(
            Identity::create(other_dir.join("other.key").to_str().unwrap())
                .await
                .unwrap(),
        );
        let object = Object::new(&other, b"borrowed bytes".to_vec(), ObjectType::Blob);
        let bundle = object.export().unwrap();
        let id = object.id.clone();

        runtime.import(bundle).await.unwrap();

        assert!(
            runtime.cache.is_cached(&id).await,
            "importing another owner's object must mark it as cached"
        );
        assert!(
            runtime.storage.exists(&id).await,
            "imported object should be on disk"
        );
    }

    #[tokio::test]
    async fn import_does_not_mark_own_object_as_cached() {
        let _guard = LOCK.lock().unwrap();
        let _ = with_scratch_home();
        let runtime = Runtime::open().await.unwrap();

        let object = runtime.put_object(b"mine".to_vec(), ObjectType::Blob, None).await.unwrap();
        let id = object.id.clone();

        // Own objects are never added to the cache index, so they can never
        // be evicted (LRU only ever yields cached entries).
        assert!(
            !runtime.cache.is_cached(&id).await,
            "objects owned by this identity must never be marked cached"
        );
        assert!(
            runtime.cache.is_owned(&object),
            "own objects must be considered owned"
        );
        assert!(
            !runtime.lru_cached().await.iter().any(|o| o.id == id),
            "own objects must not appear in the LRU eviction list"
        );
    }

    #[tokio::test]
    async fn with_root_gives_isolated_identities_and_storage() {
        let _guard = LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!(
            "canopee_runtime_with_root_{}",
            std::process::id()
        ));
        let root_a = base.join("a");
        let root_b = base.join("b");

        let runtime_a = Runtime::open_with_root(root_a.clone()).await.unwrap();
        let runtime_b = Runtime::open_with_root(root_b.clone()).await.unwrap();

        assert_ne!(
            runtime_a.identity().id(),
            runtime_b.identity().id(),
            "two runtimes with different roots must not share an identity"
        );

        let id_a = runtime_a
            .put(b"from a".to_vec(), None)
            .await
            .expect("runtime a can store");
        let id_b = runtime_b
            .put(b"from b".to_vec(), None)
            .await
            .expect("runtime b can store");
        assert_ne!(id_a, id_b);

        // Each runtime only sees its own object — they don't collide. (The
        // runtime also self-registers its device record on open, so compare
        // by object id rather than counting the store.)
        let list_a = runtime_a.list().await.unwrap();
        let list_b = runtime_b.list().await.unwrap();
        assert!(
            list_a.iter().any(|i| i.id == id_a),
            "runtime a must see its own object"
        );
        assert!(
            !list_a.iter().any(|i| i.id == id_b),
            "runtime a must not see runtime b's objects"
        );
        assert!(
            list_b.iter().any(|i| i.id == id_b),
            "runtime b must see its own object"
        );
        assert!(
            !list_b.iter().any(|i| i.id == id_a),
            "runtime b must not see runtime a's objects"
        );
        assert!(
            runtime_a.get(&id_b).await.is_err(),
            "runtime a must not see runtime b's objects"
        );

        assert!(
            root_a.join("identity/identity.key").exists(),
            "runtime a's identity key must live under its own root"
        );
        assert!(
            root_b.join("identity/identity.key").exists(),
            "runtime b's identity key must live under its own root"
        );
    }

    #[tokio::test]
    async fn evict_deletes_storage_and_cache_entry() {
        let _guard = LOCK.lock().unwrap();
        let _ = with_scratch_home();
        let runtime = Runtime::open().await.unwrap();

        let other_dir =
            std::env::temp_dir().join(format!("canopee_runtime_evict_other_{}", std::process::id()));
        std::fs::create_dir_all(&other_dir).unwrap();
        let other = Arc::new(
            Identity::create(other_dir.join("other.key").to_str().unwrap())
                .await
                .unwrap(),
        );
        let object = Object::new(&other, b"temp".to_vec(), ObjectType::Blob);
        let id = object.id.clone();
        runtime.import(object.export().unwrap()).await.unwrap();
        assert!(runtime.cache.is_cached(&id).await);

        runtime.evict(&id).await.unwrap();

        assert!(
            !runtime.cache.is_cached(&id).await,
            "evict must drop the cache index entry"
        );
        assert!(
            !runtime.storage.exists(&id).await,
            "evict must delete the object from storage"
        );
    }

    #[tokio::test]
    async fn shared_user_root_gives_one_identity_and_shared_store() {
        let _guard = LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!(
            "canopee_runtime_shared_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let user = base.join("user");
        let app_a = base.join("app-a");
        let app_b = base.join("app-b");

        let runtime_a = Runtime::open_with_config(
            Config::new().with_roots(app_a.clone(), user.clone()),
        )
        .await
        .unwrap();
        let runtime_b = Runtime::open_with_config(
            Config::new().with_roots(app_b.clone(), user.clone()),
        )
        .await
        .unwrap();

        // Same user root → same identity, so everything each app authors is
        // verifiably the same person's.
        assert_eq!(
            runtime_a.identity.id(),
            runtime_b.identity.id(),
            "shared user root must yield one identity per user"
        );
        assert_eq!(
            runtime_a.identity.dh_public_key(),
            runtime_b.identity.dh_public_key()
        );

        // A picture stored by app A is visible to app B through the shared
        // user store without any network round trip.
        let pic = runtime_a
            .put_object(b"png-bytes".to_vec(), ObjectType::Blob, None)
            .await
            .unwrap();
        assert!(
            runtime_b.storage.exists(&pic.id).await,
            "app b must see app a's picture in the shared user store"
        );

        // A contact list published by A is resolved and decoded by B through
        // the record cache (same machine) + shared store.
        let list = ContactList {
            contacts: vec![canopee_storage::Contact {
                name: "bob".into(),
                peer_id: "12D3KooB".into(),
                dh_public_key: [4u8; 32],
                note: None,
            }],
            version: 0,
        };
        runtime_a.save_contact_list(&list).await.unwrap();

        let seen = runtime_b
            .load_contact_list()
            .await
            .unwrap()
            .expect("app b must see app a's contact list");
        assert_eq!(seen.contacts[0].name, "bob");
        assert_eq!(
            seen.contacts[0].dh_public_key,
            list.contacts[0].dh_public_key
        );

        // App-scoped state is still isolated: each app has its own state file.
        assert_ne!(
            runtime_a.config.state_path(),
            runtime_b.config.state_path()
        );
        assert!(runtime_a.config.state_path().starts_with(&app_a));
        assert!(runtime_b.config.state_path().starts_with(&app_b));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn evict_owned_object_is_allowed_only_through_explicit_evict() {
        let _guard = LOCK.lock().unwrap();
        let _ = with_scratch_home();
        let runtime = Runtime::open().await.unwrap();

        let mine = runtime.put_object(b"keep me".to_vec(), ObjectType::Blob, None).await.unwrap();
        assert!(runtime.cache.is_owned(&mine));
        // Owned objects are never *auto-marked* cached, so a plain eviction
        // path won't touch them (LRU only yields cached entries).
        assert!(!runtime.lru_cached().await.iter().any(|o| o.id == mine.id));
    }

    /// Builds the same provider the runtime's network layer serves through,
    /// so tests can exercise the sharing gate without a swarm.
    fn provider_for(runtime: &Runtime) -> StorageObjectProvider {
        StorageObjectProvider {
            storage: runtime.storage.clone(),
            cache: runtime.cache.clone(),
            shared: runtime.shared.clone(),
        }
    }

    #[tokio::test]
    async fn owned_objects_are_served_only_once_shared() {
        let _guard = LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!(
            "canopee_runtime_gate_owned_{}",
            std::process::id()
        ));
        let runtime = Runtime::open_with_root(base).await.unwrap();
        let provider = provider_for(&runtime);

        let object = runtime
            .put_object(b"private bytes".to_vec(), ObjectType::Blob, None)
            .await
            .unwrap();

        assert!(
            provider.get_object(&object.id).await.is_none(),
            "owned + unshared must not be served to the network"
        );
        assert!(!runtime.is_shared(&object.id).await);

        runtime.mark_shared(&object.id).await;

        let served = provider
            .get_object(&object.id)
            .await
            .expect("owned + shared must be served");
        assert_eq!(served.object.payload.data, b"private bytes");
        assert!(runtime.is_shared(&object.id).await);
    }

    #[tokio::test]
    async fn cached_objects_are_served_without_sharing() {
        let _guard = LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!(
            "canopee_runtime_gate_cached_{}",
            std::process::id()
        ));
        let runtime = Runtime::open_with_root(base).await.unwrap();
        let provider = provider_for(&runtime);

        // An object signed by someone else, imported from the network.
        let other_dir = std::env::temp_dir().join(format!(
            "canopee_runtime_gate_cached_other_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&other_dir).unwrap();
        let other = Arc::new(
            Identity::create(other_dir.join("other.key").to_str().unwrap())
                .await
                .unwrap(),
        );
        let object = Object::new(&other, b"public bytes".to_vec(), ObjectType::Blob);
        let id = object.id.clone();
        runtime.import(object.export().unwrap()).await.unwrap();
        assert!(runtime.cache.is_cached(&id).await);

        let served = provider
            .get_object(&id)
            .await
            .expect("cached objects must be re-served (distributed cache)");
        assert_eq!(served.object.payload.data, b"public bytes");
    }

    #[tokio::test]
    async fn foreign_uncached_objects_are_not_served() {
        let _guard = LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!(
            "canopee_runtime_gate_foreign_{}",
            std::process::id()
        ));
        let runtime = Runtime::open_with_root(base).await.unwrap();
        let provider = provider_for(&runtime);

        // A foreign object written straight into storage, bypassing
        // `Runtime::import` (so it is neither owned nor cached).
        let other_dir = std::env::temp_dir().join(format!(
            "canopee_runtime_gate_foreign_other_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&other_dir).unwrap();
        let other = Arc::new(
            Identity::create(other_dir.join("other.key").to_str().unwrap())
                .await
                .unwrap(),
        );
        let object = Object::new(&other, b"smuggled bytes".to_vec(), ObjectType::Blob);
        let id = object.id.clone();
        runtime.storage.put_verified(&object).await.unwrap();
        assert!(!runtime.cache.is_cached(&id).await);

        assert!(
            provider.get_object(&id).await.is_none(),
            "an object that is neither owned-and-shared nor cached must not be served"
        );
    }

    #[tokio::test]
    async fn save_home_index_reconciles_shared_flags() {
        let _guard = LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!(
            "canopee_runtime_reconcile_{}",
            std::process::id()
        ));
        let runtime = Runtime::open_with_root(base).await.unwrap();

        let blob = runtime
            .put_object(b"pic".to_vec(), ObjectType::Blob, None)
            .await
            .unwrap();
        let entry = canopee_storage::HomeEntry {
            name: "pic.png".into(),
            object: blob.id.clone(),
            object_type: ObjectType::Blob,
            shared: true,
            app: Some("test".into()),
        };
        let index = HomeIndex {
            profile: None,
            contacts: None,
            entries: vec![entry.clone()],
            version: 0,
        };
        runtime.save_home_index(&index).await.unwrap();
        assert!(
            runtime.is_shared(&blob.id).await,
            "saving an index with shared: true must mark the object shared"
        );

        // Flip the entry back to private and resave: the share is withdrawn.
        let mut entry = entry;
        entry.shared = false;
        let index = HomeIndex {
            profile: None,
            contacts: None,
            entries: vec![entry],
            version: 0,
        };
        runtime.save_home_index(&index).await.unwrap();
        assert!(
            !runtime.is_shared(&blob.id).await,
            "resaving with shared: false must withdraw the object"
        );
    }

    #[tokio::test]
    async fn set_home_entry_shared_flips_flag_and_validates() {
        let _guard = LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!(
            "canopee_runtime_set_shared_{}",
            std::process::id()
        ));
        let runtime = Runtime::open_with_root(base).await.unwrap();

        // No home index yet.
        assert!(runtime.set_home_entry_shared("pic.png", true).await.is_err());

        let blob = runtime
            .put_object(b"pic".to_vec(), ObjectType::Blob, None)
            .await
            .unwrap();
        let index = HomeIndex {
            profile: None,
            contacts: None,
            entries: vec![canopee_storage::HomeEntry {
                name: "pic.png".into(),
                object: blob.id.clone(),
                object_type: ObjectType::Blob,
                shared: false,
                app: None,
            }],
            version: 0,
        };
        runtime.save_home_index(&index).await.unwrap();
        assert!(!runtime.is_shared(&blob.id).await);

        // Unknown entry name.
        assert!(runtime.set_home_entry_shared("nope", true).await.is_err());

        runtime.set_home_entry_shared("pic.png", true).await.unwrap();
        assert!(runtime.is_shared(&blob.id).await);
        let index = runtime.load_home_index().await.unwrap().unwrap();
        assert!(index.entries[0].shared);

        runtime.set_home_entry_shared("pic.png", false).await.unwrap();
        assert!(!runtime.is_shared(&blob.id).await);
        let index = runtime.load_home_index().await.unwrap().unwrap();
        assert!(!index.entries[0].shared);
    }

    #[tokio::test]
    async fn share_object_upserts_entry_and_serves() {
        let _guard = LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!(
            "canopee_runtime_share_object_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let runtime = Runtime::open_with_root(base).await.unwrap();

        // Sharing an object that isn't stored locally is an error.
        assert!(
            runtime
                .share_object("ghost", &ObjectId::new("nope"), None)
                .await
                .is_err()
        );

        let blob = runtime
            .put_object(b"share me".to_vec(), ObjectType::Blob, None)
            .await
            .unwrap();

        // First share creates the home index with one shared entry.
        runtime
            .share_object("file.txt", &blob.id, Some("cli".into()))
            .await
            .unwrap();
        assert!(runtime.is_shared(&blob.id).await);
        let index = runtime.load_home_index().await.unwrap().unwrap();
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].name, "file.txt");
        assert_eq!(index.entries[0].object, blob.id);
        assert!(index.entries[0].shared);
        assert_eq!(index.entries[0].app.as_deref(), Some("cli"));

        // Re-sharing the same name updates the entry in place rather than
        // duplicating it.
        let blob2 = runtime
            .put_object(b"new version".to_vec(), ObjectType::Blob, None)
            .await
            .unwrap();
        runtime.share_object("file.txt", &blob2.id, None).await.unwrap();
        let index = runtime.load_home_index().await.unwrap().unwrap();
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].object, blob2.id);
        assert!(runtime.is_shared(&blob2.id).await);

        // Unsharing withdraws the current object.
        runtime.set_home_entry_shared("file.txt", false).await.unwrap();
        assert!(!runtime.is_shared(&blob2.id).await);
    }

    #[tokio::test]
    async fn sharing_publishes_a_resolvable_entry_pointer() {
        let _guard = LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!(
            "canopee_runtime_entry_pointer_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let runtime = Runtime::open_with_root(base).await.unwrap();

        let blob = runtime
            .put_object(b"pointer me".to_vec(), ObjectType::Blob, None)
            .await
            .unwrap();
        let owner = runtime.identity().id().clone();

        // Sharing under a name must make that name resolvable from the local
        // record cache (the same machine is authoritative instantly — this is
        // the path Bob's `canopee fetch alice <name>` uses).
        runtime.share_object("summer-mix", &blob.id, None).await.unwrap();
        let record = runtime
            .resolve_pointer(&owner, "entry:summer-mix")
            .await
            .unwrap()
            .expect("share must publish the (owner, \"entry:<name>\") pointer");
        assert_eq!(record.manifest, blob.id);

        // Unsharing must not have published a pointer for an entry that was
        // never shared — private entries never surface as records.
        let private = runtime
            .put_object(b"private".to_vec(), ObjectType::Blob, None)
            .await
            .unwrap();
        assert!(
            runtime
                .resolve_pointer(&owner, "entry:private")
                .await
                .unwrap()
                .is_none(),
            "unshared entries must not get pointer records"
        );
        let _ = private;
    }

    #[tokio::test]
    async fn shared_entries_survive_reopen() {
        let _guard = LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!(
            "canopee_runtime_reopen_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);

        let blob_id = {
            let runtime = Runtime::open_with_root(base.clone()).await.unwrap();
            let blob = runtime
                .put_object(b"durable".to_vec(), ObjectType::Blob, None)
                .await
                .unwrap();
            let index = HomeIndex {
                profile: None,
                contacts: None,
                entries: vec![canopee_storage::HomeEntry {
                    name: "durable.bin".into(),
                    object: blob.id.clone(),
                    object_type: ObjectType::Blob,
                    shared: true,
                    app: None,
                }],
                version: 0,
            };
            runtime.save_home_index(&index).await.unwrap();
            assert!(runtime.is_shared(&blob.id).await);
            blob.id
        };

        // Reopen the same root: the persisted home index re-seeds the
        // serving set, so the share survives the restart.
        let runtime = Runtime::open_with_root(base).await.unwrap();
        assert!(
            runtime.is_shared(&blob_id).await,
            "a shared: true entry must still be served after a restart"
        );
        let provider = provider_for(&runtime);
        assert!(provider.get_object(&blob_id).await.is_some());
    }

    #[tokio::test]
    async fn peers_can_fetch_shared_objects_but_not_private_ones() {
        let _guard = LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!(
            "canopee_runtime_netshare_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let runtime_a = Runtime::open_with_root(base.join("a")).await.unwrap();
        let runtime_b = Runtime::open_with_root(base.join("b")).await.unwrap();
        let peer_a = PeerId::from(runtime_a.identity.keypair().public());

        let public = runtime_a
            .put_object(b"for the network".to_vec(), ObjectType::Blob, None)
            .await
            .unwrap();
        let private = runtime_a
            .put_object(b"not for the network".to_vec(), ObjectType::Blob, None)
            .await
            .unwrap();

        // Connect B to A and wait until the connection is up.
        let addr_a = loop {
            let addrs = runtime_a.network.listen_addresses().await.unwrap();
            if let Some(a) = addrs.iter().find(|a| a.to_string().contains("/tcp/")) {
                break a.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        };
        runtime_b.network.dial(addr_a).await.unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            if runtime_b
                .network
                .peers()
                .await
                .unwrap_or_default()
                .iter()
                .any(|p| p.peer_id == peer_a)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // A's private object is stored locally but never shared: B's direct
        // request is refused.
        let refused = runtime_b
            .network
            .get_object(peer_a, private.id.clone())
            .await;
        assert!(
            refused.is_err(),
            "a private (never-shared) object must not be fetchable"
        );

        // Once A announces the public object, B discovers and fetches it via
        // the DHT — and, being an import, B's copy becomes cached and
        // re-servable to further peers.
        runtime_a.announce(public.id.clone()).await.unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        let fetched = loop {
            match runtime_b.fetch_object(public.id.clone(), None).await {
                Ok(object) => break object,
                Err(_) => {
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "timed out fetching announced object"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        };
        assert_eq!(fetched.payload.data, b"for the network");
        assert!(
            runtime_b.cache.is_cached(&public.id).await,
            "fetching an announced object imports it as cached"
        );
        let provider_b = provider_for(&runtime_b);
        assert!(
            provider_b.get_object(&public.id).await.is_some(),
            "a cached copy must be re-served to further peers"
        );

        // And B's own unshared objects stay private the whole time.
        assert!(
            provider_b.get_object(&private.id).await.is_none(),
            "B does not even have A's private object"
        );
        let b_private = runtime_b
            .put_object(b"b's secret".to_vec(), ObjectType::Blob, None)
            .await
            .unwrap();
        assert!(
            provider_b.get_object(&b_private.id).await.is_none(),
            "B's own unshared object must not be served"
        );
    }
}
