mod state;
use canopee_config::Config;
use canopee_identity::{Identity, IdentityId};
use canopee_network::{Multiaddr, NetworkManager, ObjectProvider, PeerId};
use canopee_storage::{
    AppPointerRecord, Cache, CacheIndex, ContactList, Export, ExportBundle, HomeIndex, Object,
    ObjectId, ObjectInfo, ObjectType, Profile, UsernameRecord, RECORD_CONTACTS, RECORD_HOME,
    RECORD_PROFILE, RECORD_USERNAME, USERNAME_REGISTRY_PREFIX, Storage,
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

pub struct Runtime {
    pub config: Config,
    pub identity: Arc<Identity>,
    pub storage: Arc<Storage>,
    pub network: NetworkManager,
    pub cache: Arc<Cache>,
    state: RwLock<NodeState>,
    shared: SharedSet,
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
        let network = NetworkManager::new(
            identity.clone(),
            listen_addr,
            object_provider,
            config.mdns_enabled(),
        )?;

        let runtime = Self {
            config,
            identity,
            storage,
            network,
            cache,
            state: RwLock::new(state),
            shared,
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

        Ok(runtime)
    }

    /// Re-announces the objects the local `(owner, "profile")` and
    /// `(owner, "username")` records point at, if any. Reads the local
    /// record cache only (no DHT lookups) so startup stays fast; failures
    /// are logged and skipped.
    async fn reshare_public_user_records(&self) {
        for name in [RECORD_PROFILE, RECORD_USERNAME] {
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

    pub async fn put(&self, data: Vec<u8>) -> anyhow::Result<ObjectId> {
        let object = Object::new(&self.identity, data, ObjectType::Blob);
        let id = object.id.clone();
        self.storage.put_verified(&object).await?;

        Ok(id)
    }

    pub async fn put_object(
        &self,
        data: Vec<u8>,
        object_type: ObjectType,
    ) -> anyhow::Result<Object> {
        let object = Object::new(&self.identity(), data, object_type);

        self.storage.put_verified(&object).await?;

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
    /// resolve its signed username and profile display name from the network
    /// and push them back into the network manager's peer map, so `peers()`
    /// and the UI can show friendly names instead of raw peer ids.
    ///
    /// Resolution is opportunistic: peers whose records can't be reached
    /// (offline DHT, not connected to bootstrap peers) simply keep `None` for
    /// the unknown fields. All peers are resolved *concurrently*, each with
    /// its own timeout, so one peer with no records (e.g. a bootstrap relay)
    /// can't starve the others or discard their results.
    pub async fn enrich_peers(&self) -> anyhow::Result<()> {
        const ENRICH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

        let peers = self.network.peers().await?;
        let resolutions = peers.into_iter().filter_map(|peer| {
            let identity = peer.identity?;
            // Per-peer timeout (not one shared deadline): a peer with no
            // records (e.g. a bootstrap relay) times out on its own without
            // discarding results already resolved for everyone else.
            Some(async move {
                // Each record gets its OWN timeout inside the join: a peer
                // with a username but no profile must not lose its resolved
                // username just because the profile DHT lookup is slow to
                // return not-found.
                let (username, display_name) = tokio::join!(
                    async {
                        tokio::time::timeout(ENRICH_TIMEOUT, self.resolve_username(&identity))
                            .await
                            .ok()
                            .and_then(|r| r.ok())
                            .flatten()
                            .map(|u| u.username)
                    },
                    async {
                        tokio::time::timeout(ENRICH_TIMEOUT, self.resolve_profile(&identity))
                            .await
                            .ok()
                            .and_then(|r| r.ok())
                            .flatten()
                            .map(|p| p.display_name)
                    }
                );
                (identity, username, display_name)
            })
        });
        let results = futures::future::join_all(resolutions).await;
        for (identity, username, display_name) in results {
            if username.is_none() && display_name.is_none() {
                continue;
            }
            if let Some(peer_id) = owner_peer_id(&identity) {
                let _ = self
                    .network
                    .set_peer_meta(peer_id, Some(identity), username, display_name)
                    .await;
            }
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
        entry.shared = shared;
        self.save_home_index(&index).await
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
        self.save_home_index(&index).await
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

        let object = runtime.put_object(b"mine".to_vec(), ObjectType::Blob).await.unwrap();
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
            .put(b"from a".to_vec())
            .await
            .expect("runtime a can store");
        let id_b = runtime_b
            .put(b"from b".to_vec())
            .await
            .expect("runtime b can store");
        assert_ne!(id_a, id_b);

        // Each runtime only sees its own object — they don't collide.
        assert_eq!(runtime_a.list().await.unwrap().len(), 1);
        assert_eq!(runtime_b.list().await.unwrap().len(), 1);
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
            .put_object(b"png-bytes".to_vec(), ObjectType::Blob)
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

        let mine = runtime.put_object(b"keep me".to_vec(), ObjectType::Blob).await.unwrap();
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
            .put_object(b"private bytes".to_vec(), ObjectType::Blob)
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
            .put_object(b"pic".to_vec(), ObjectType::Blob)
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
            .put_object(b"pic".to_vec(), ObjectType::Blob)
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
            .put_object(b"share me".to_vec(), ObjectType::Blob)
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
            .put_object(b"new version".to_vec(), ObjectType::Blob)
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
                .put_object(b"durable".to_vec(), ObjectType::Blob)
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
            .put_object(b"for the network".to_vec(), ObjectType::Blob)
            .await
            .unwrap();
        let private = runtime_a
            .put_object(b"not for the network".to_vec(), ObjectType::Blob)
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
            .put_object(b"b's secret".to_vec(), ObjectType::Blob)
            .await
            .unwrap();
        assert!(
            provider_b.get_object(&b_private.id).await.is_none(),
            "B's own unshared object must not be served"
        );
    }
}
