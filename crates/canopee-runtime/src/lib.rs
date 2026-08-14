mod state;
use canopee_config::Config;
use canopee_identity::Identity;
use canopee_network::{Multiaddr, NetworkManager, ObjectProvider};
use canopee_storage::{Export, ExportBundle, Object, ObjectId, ObjectInfo, ObjectType, Storage};
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
    state: RwLock<NodeState>,
}

struct StorageObjectProvider {
    storage: Arc<Storage>,
}

#[async_trait::async_trait]
impl ObjectProvider for StorageObjectProvider {
    async fn get_object(&self, id: &ObjectId) -> Option<ExportBundle> {
        let object = self.storage.get_verified(id).await.ok()?;
        object.export().ok()
    }
}

impl Runtime {
    pub async fn open() -> anyhow::Result<Self> {
        // init configs...
        let config = Config::new();

        // init home root directory...
        let root = config.home_dir();
        tokio::fs::create_dir_all(&root).await?;

        // init identity root directory...
        let identity_dir = config.identity_path();
        tokio::fs::create_dir_all(&identity_dir).await?;

        // load or create root identity cryptographic key...
        let identity_path = identity_dir.join("identity.key");
        let identity = match Identity::load(identity_path.to_str().unwrap()).await {
            Ok(id) => id,
            Err(_) => Identity::create(identity_path.to_str().unwrap()).await?,
        };

        // init or use state root directory...
        let state_path = config.state_path();
        tokio::fs::create_dir_all(state_path.parent().unwrap()).await?;

        // load or initialize new Node state...
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

        // init storage roor directory...
        let storage_path = config.storage_path();
        tokio::fs::create_dir_all(&storage_path).await?;

        let storage = Arc::new(Storage::new(storage_path.to_str().unwrap()));
        let identity = Arc::new(identity);

        // init Runtime connection listener...
        let listen_addr: Multiaddr = config.listen_addr().parse()?;
        let object_provider = Arc::new(StorageObjectProvider {
            storage: storage.clone(),
        });

        // init Noetwork node...
        let network = NetworkManager::new(identity.clone(), listen_addr, object_provider)?;

        Ok(Self {
            config,
            identity,
            storage,
            network,
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
        println!("GET IS CALLED HERE");
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
        self.storage.import(&export_bundle.object).await?;

        Ok(())
    }
}
