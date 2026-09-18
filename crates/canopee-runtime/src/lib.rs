mod capabilities;
mod pairing;
mod records;
mod sharing;
mod state;
mod sync;

use canopee_config::Config;
use canopee_identity::{DeviceKey, Identity, IdentityId};
use canopee_network::{Multiaddr, NetworkManager, ObjectProvider};
use canopee_storage::{
    Cache, CacheIndex, Export, ExportBundle, Object, ObjectId, ObjectInfo, ObjectType,
    Storage,
};
use state::NodeState;
use std::path::PathBuf;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::RwLock;

pub(crate) fn normalize_username(username: &str) -> anyhow::Result<String> {
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

pub(crate) fn owner_peer_id(owner: &IdentityId) -> Option<canopee_network::PeerId> {
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
    pub(crate) state: Arc<RwLock<NodeState>>,
    pub(crate) shared: SharedSet,
    /// The in-flight LAN pairing session on THIS device (the new device):
    /// the 12-char code + one-time session id minted by `initiate_pairing`,
    /// held only in memory until the inbound request arrives.
    pub(crate) pairing: Arc<RwLock<Option<OwnPairing>>>,
}

#[derive(Clone)]
pub(crate) struct OwnPairing {
    pub(crate) session_id: String,
    pub(crate) code: String,
}

/// The set of objects this node explicitly serves to the network. In-memory
/// by design: DHT provider records are themselves ephemeral (re-announced
/// per session), so the servable set matches their lifetime.
#[derive(Clone, Default)]
pub(crate) struct SharedSet {
    ids: Arc<RwLock<std::collections::HashSet<String>>>,
}

impl SharedSet {
    pub(crate) async fn contains(&self, id: &ObjectId) -> bool {
        self.ids.read().await.contains(&id.0)
    }

    pub(crate) async fn insert(&self, id: &ObjectId) {
        self.ids.write().await.insert(id.0.clone());
    }

    pub(crate) async fn remove(&self, id: &ObjectId) {
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
        if self.cache.is_cached(id).await {
            self.cache.touch_served(id).await;
            return object.export().ok();
        }
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
    /// mDNS is disabled by default: several apps may be open at once, and a
    /// shared identity must never be re-announced over multicast by each
    /// swarm.
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

        if let Ok(Some(index)) = runtime.load_home_index().await {
            runtime.reconcile_shared(None, Some(&index)).await;
        }

        runtime.reshare_public_user_records().await;
        runtime.register_device().await;
        runtime.schedule_device_publish();
        runtime.spawn_pairing_handler();
        runtime.spawn_periodic_sync();

        Ok(runtime)
    }

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

    async fn reshare_public_user_records(&self) {
        use canopee_storage::{AppPointerRecord, RECORD_DEVICES, RECORD_PROFILE, RECORD_USERNAME};
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

    pub fn export_identity(&self, passphrase: &str) -> anyhow::Result<Vec<u8>> {
        self.identity.export_encrypted(passphrase)
    }

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
    /// index, and withdraws this node as a DHT provider of it.
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
}

// DHT timeout shared by pointer resolution (records) and record sync (sync).
pub(crate) const RESOLVE_DHT_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(10);

#[cfg(test)]
mod tests {
    use super::*;
    use canopee_identity::Identity;
    use canopee_storage::ObjectType;
    use std::sync::Once;

    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static SETUP: Once = Once::new();

    fn with_scratch_home() -> &'static PathBuf {
        SETUP.call_once(|| {
            let dir = std::env::temp_dir().join(format!(
                "canopee_runtime_test_{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
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

        assert_eq!(
            runtime_a.identity.id(),
            runtime_b.identity.id(),
            "shared user root must yield one identity per user"
        );
        assert_eq!(
            runtime_a.identity.dh_public_key(),
            runtime_b.identity.dh_public_key()
        );

        let pic = runtime_a
            .put_object(b"png-bytes".to_vec(), ObjectType::Blob, None)
            .await
            .unwrap();
        assert!(
            runtime_b.storage.exists(&pic.id).await,
            "app b must see app a's picture in the shared user store"
        );

        let list = canopee_storage::ContactList {
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
        assert!(!runtime.lru_cached().await.iter().any(|o| o.id == mine.id));
    }

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
            .put_object(b"private".to_vec(), ObjectType::Blob, None)
            .await
            .unwrap();

        // Not shared yet — the provider must refuse to serve it.
        assert!(
            provider.get_object(&object.id).await.is_none(),
            "unshared object must not be served"
        );

        // Explicitly share: now the provider should serve it.
        runtime.announce(object.id.clone()).await.unwrap();
        assert!(
            provider.get_object(&object.id).await.is_some(),
            "shared object must be served"
        );

        // Unshare: the provider must again refuse.
        runtime.shared.remove(&object.id).await;
        assert!(
            provider.get_object(&object.id).await.is_none(),
            "un-shared object must not be served"
        );
    }

    #[tokio::test]
    async fn cached_objects_are_always_served() {
        let _guard = LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!(
            "canopee_runtime_gate_cached_{}",
            std::process::id()
        ));
        let runtime = Runtime::open_with_root(base).await.unwrap();
        let provider = provider_for(&runtime);

        // Import an object from another identity — it becomes cached.
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
        let object = Object::new(&other, b"from peer".to_vec(), ObjectType::Blob);
        let id = object.id.clone();
        let bundle = object.export().unwrap();
        runtime.import(bundle).await.unwrap();
        assert!(runtime.cache.is_cached(&id).await);

        assert!(
            provider.get_object(&id).await.is_some(),
            "cached object must always be served"
        );
    }

    #[tokio::test]
    async fn home_index_share_unshare_roundtrip() {
        let _guard = LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!(
            "canopee_runtime_home_{}",
            std::process::id()
        ));
        let runtime = Runtime::open_with_root(base).await.unwrap();
        let provider = provider_for(&runtime);

        let id = runtime.put(b"file".to_vec(), None).await.unwrap();

        // Initially unshared.
        assert!(
            provider.get_object(&id).await.is_none(),
            "must not be served before sharing"
        );

        // Share via home index.
        runtime
            .share_object("photo", &id, None)
            .await
            .unwrap();
        assert!(
            provider.get_object(&id).await.is_some(),
            "must be served after sharing"
        );

        // Unshare via home index.
        runtime.set_home_entry_shared("photo", false).await.unwrap();
        assert!(
            provider.get_object(&id).await.is_none(),
            "must not be served after unsharing"
        );
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
        let index = canopee_storage::HomeIndex {
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
        let index = canopee_storage::HomeIndex {
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
        let index = canopee_storage::HomeIndex {
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
        let provider = provider_for(&runtime);

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
        assert!(
            provider.get_object(&blob.id).await.is_none(),
            "an unshared object must not be served"
        );

        // First share creates the home index with one shared entry.
        runtime
            .share_object("file.txt", &blob.id, Some("cli".into()))
            .await
            .unwrap();
        assert!(runtime.is_shared(&blob.id).await);
        assert!(
            provider.get_object(&blob.id).await.is_some(),
            "a shared object must be served"
        );
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
        assert!(
            provider.get_object(&blob2.id).await.is_none(),
            "an un-shared object must not be served"
        );
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
            let index = canopee_storage::HomeIndex {
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
        let peer_a = canopee_network::PeerId::from(runtime_a.identity.keypair().public());

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
