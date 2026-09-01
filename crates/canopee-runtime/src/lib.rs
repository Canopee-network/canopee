mod state;
use canopee_config::Config;
use canopee_identity::Identity;
use canopee_network::{Multiaddr, NetworkManager, ObjectProvider};
use canopee_storage::{Cache, CacheIndex, Export, ExportBundle, Object, ObjectId, ObjectInfo, ObjectType, Storage};
use state::NodeState;
use std::path::PathBuf;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::RwLock;

pub struct Runtime {
    pub config: Config,
    pub identity: Arc<Identity>,
    pub storage: Arc<Storage>,
    pub network: NetworkManager,
    pub cache: Arc<Cache>,
    state: RwLock<NodeState>,
}

struct StorageObjectProvider {
    storage: Arc<Storage>,
    cache: Arc<Cache>,
}

#[async_trait::async_trait]
impl ObjectProvider for StorageObjectProvider {
    async fn get_object(&self, id: &ObjectId) -> Option<ExportBundle> {
        let object = self.storage.get_verified(id).await.ok()?;
        // Touch the cache timestamp for cached (non-owned) objects when
        // serving them to another peer — this is the "someone is actually
        // using my cached copy" signal that drives LRU eviction.
        if self.cache.is_cached(id).await {
            self.cache.touch_served(id).await;
        }
        object.export().ok()
    }
}

impl Runtime {
    pub async fn open() -> anyhow::Result<Self> {
        let config = Config::new();
        let root = config.home_dir();
        tokio::fs::create_dir_all(&root).await?;
        let identity_dir = config.identity_path();
        tokio::fs::create_dir_all(&identity_dir).await?;
        let identity_path = identity_dir.join("identity.key");
        let identity = match Identity::load(identity_path.to_str().unwrap()).await {
            Ok(id) => id,
            Err(_) => Identity::create(identity_path.to_str().unwrap()).await?,
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
        let object_provider = Arc::new(StorageObjectProvider {
            storage: storage.clone(),
            cache: cache.clone(),
        });
        let network = NetworkManager::new(identity.clone(), listen_addr, object_provider)?;

        Ok(Self {
            config,
            identity,
            storage,
            network,
            cache,
            state: RwLock::new(state),
        })
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
}
