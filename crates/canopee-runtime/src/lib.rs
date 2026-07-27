mod config;
use canopee_identity::Identity;
use canopee_storage::{Export, ExportBundle, Object, ObjectId, ObjectInfo, Storage};
use config::Config;
use std::path::PathBuf;

pub struct Runtime {
    pub config: Config,
    pub identity: Identity,
    pub storage: Storage,
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
        let storage_path = config.storage_path();
        tokio::fs::create_dir_all(&storage_path).await?;
        let storage = Storage::new(storage_path.to_str().unwrap());

        Ok(Self {
            config,
            identity,
            storage,
        })
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        println!("Canopee node running");
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    }

    pub fn export_path(&self) -> PathBuf {
        self.config.export_path()
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
        let object = Object::new(&self.identity, data);
        let id = object.id.clone();
        self.storage.put_verified(&object).await?;

        Ok(id)
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
        self.storage.import(&export_bundle.object).await?;

        Ok(())
    }
}
