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
        // Write to a temp name then rename, so a concurrent writer (multiple
        // apps sharing one *user* store) can never leave a torn file behind.
        // Object ids are content-addressed, so every writer of a given id
        // writes identical bytes and concurrent renames are idempotent.
        let tmp = format!("{}/.{}.canopee-tmp", self.root, object.id.0);
        let bytes = bincode::serialize(object)?;
        fs::write(&tmp, bytes).await?;
        fs::rename(&tmp, path).await?;
        Ok(())
    }

    pub async fn get_verified(&self, id: &ObjectId) -> Result<Object> {
        let path = format!("{}/{}", self.root, id.0);
        let bytes = fs::read(path).await?;
        let object: Object = bincode::deserialize(&bytes)?;
        // The file path must name the object it holds. A content-addressed id
        // pins this: an object written under someone else's id (a wrongly
        // renamed file, or a stale temp surviving a rename) must never be
        // returned as that other object.
        if object.id != *id {
            anyhow::bail!(
                "Object id mismatch: file holds {}, requested {}",
                object.id,
                id
            );
        }
        if !object.verify() {
            anyhow::bail!("Invalid signature for object {}", id);
        }
        Ok(object)
    }

    pub async fn exists(&self, id: &ObjectId) -> bool {
        let path = format!("{}/{}", self.root, id.0);
        fs::try_exists(path).await.unwrap_or(false)
    }

    pub async fn list(&self) -> Result<Vec<ObjectId>> {
        let mut objects = Vec::new();
        let mut entries = fs::read_dir(&self.root).await?;
        while let Some(entry) = entries.next_entry().await? {
            let filename = entry.file_name().to_string_lossy().to_string();
            // Skip temp files and the record cache, which share the same
            // user-root layout.
            if filename.starts_with('.') {
                continue;
            }
            objects.push(ObjectId::new(&filename));
        }
        Ok(objects)
    }

    pub async fn list_objects(&self) -> Result<Vec<ObjectInfo>> {
        let mut objects = Vec::new();
        let mut entries = fs::read_dir(&self.root).await?;
        while let Some(entry) = entries.next_entry().await? {
            let filename = entry.file_name().to_string_lossy().to_string();
            if filename.starts_with('.') {
                continue;
            }
            let bytes = fs::read(entry.path()).await?;
            // A file that fails to decode is treated as absent rather than
            // failing the whole listing — protects concurrent writers in a
            // shared user store from transiently breaking every other app's
            // "my data" view.
            let Ok(object) = bincode::deserialize::<Object>(&bytes) else {
                continue;
            };
            let verified = object.verify();
            let id = object.id.clone();
            let object_info = ObjectInfo {
                id: id.clone(),
                owner: object.payload.owner,
                size: object.payload.metadata.size,
                verified,
                name: self.read_name(&id).await,
            };
            objects.push(object_info);
        }
        Ok(objects)
    }

    pub async fn import(&self, object: &Object) -> Result<()> {
        self.put_verified(object).await
    }

    /// Records a human-friendly name for an object (e.g. the file name it was
    /// stored from), stored *alongside* the object rather than inside its
    /// signed payload. Keeping it out of the content-addressed object means
    /// object ids and the on-disk format never change, old objects keep
    /// listing, and re-putting identical bytes still yields the same id.
    pub async fn set_name(&self, id: &ObjectId, name: &str) -> Result<()> {
        let path = self.name_path(id);
        fs::write(path, name.as_bytes()).await?;
        Ok(())
    }

    /// Reads the recorded name for `id`, if any. Corrupt sidecars are treated
    /// as absent rather than failing the listing.
    pub async fn read_name(&self, id: &ObjectId) -> Option<String> {
        let bytes = match fs::read(self.name_path(id)).await {
            Ok(bytes) => bytes,
            Err(_) => return None,
        };
        String::from_utf8(bytes).ok().filter(|s| !s.is_empty())
    }

    fn name_path(&self, id: &ObjectId) -> std::path::PathBuf {
        std::path::Path::new(&self.root).join(format!(".{}.name", id.0))
    }

    /// Removes a single object from storage by id. No-op if it doesn't exist.
    pub async fn delete(&self, id: &ObjectId) -> Result<()> {
        let path = format!("{}/{}", self.root, id.0);
        // Missing is not an error — eviction races with other writers.
        if fs::try_exists(&path).await.unwrap_or(false) {
            fs::remove_file(path).await?;
        }
        // Drop the name sidecar too, so an evicted object doesn't leave
        // orphaned metadata behind.
        let name = self.name_path(id);
        if fs::try_exists(&name).await.unwrap_or(false) {
            fs::remove_file(name).await?;
        }
        Ok(())
    }
}
