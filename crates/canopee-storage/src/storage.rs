use crate::object_id::ObjectId;
use crate::{Object, ObjectInfo, Verify};
use anyhow::Result;
use tokio::fs;

pub struct Storage {
    root: String,
}

impl Storage {
    pub fn new(root: &str) -> Self {
        Self {
            root: root.to_string(),
        }
    }

    pub fn root(&self) -> String {
        self.root.clone()
    }

    pub async fn put_verified(&self, object: &Object) -> Result<()> {
        if !object.verify() {
            anyhow::bail!("Invalid signature for object {}", object.id);
        }
        let path = format!("{}/{}", self.root, object.id.0);
        let bytes = bincode::serialize(object)?;
        fs::write(path, bytes).await?;
        Ok(())
    }

    pub async fn get_verified(&self, id: &ObjectId) -> Result<Object> {
        let path = format!("{}/{}", self.root, id.0);
        let bytes = fs::read(path).await?;
        let object: Object = bincode::deserialize(&bytes)?;
        if !object.verify() {
            anyhow::bail!("Invalid signature for object {}", id);
        }

        Ok(object)
    }

    pub async fn exists(&self, id: &ObjectId) -> bool {
        let path = format!("{}/{}", self.root, id.0);
        fs::try_exists(path).await.is_ok()
    }

    pub async fn list(&self) -> Result<Vec<ObjectId>> {
        let mut objects = Vec::new();
        let mut entries = fs::read_dir(&self.root).await?;
        while let Some(entry) = entries.next_entry().await? {
            let filename = entry.file_name().to_string_lossy().to_string();
            objects.push(ObjectId::new(&filename));
        }
        Ok(objects)
    }

    pub async fn list_objects(&self) -> Result<Vec<ObjectInfo>> {
        let mut objects = Vec::new();
        let mut entries = fs::read_dir(&self.root).await?;
        while let Some(entry) = entries.next_entry().await? {
            let bytes = fs::read(entry.path()).await?;
            let object = bincode::deserialize::<Object>(&bytes)?;
            let verified = object.verify();
            let object_info = ObjectInfo {
                id: object.id,
                owner: object.payload.owner,
                size: object.payload.metadata.size,
                verified,
            };
            objects.push(object_info);
        }
        Ok(objects)
    }

    pub async fn import(&self, object: &Object) -> Result<()> {
        self.put_verified(object).await
    }
}
