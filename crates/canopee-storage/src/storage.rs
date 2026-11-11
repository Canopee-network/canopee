use crate::object_id::ObjectId;
use crate::{Object, ObjectInfo, Verify};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};
use std::time::SystemTime;
use tokio::fs;

/// Cheap on-disk stamp for an object file: anything that changes the parsed
/// object (its bytes, and therefore its size or content) also changes one of
/// these. A listing re-reads and re-verifies an object only when its stamp
/// differs from the cached one.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl FileStamp {
    fn of(metadata: &std::fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

struct CachedInfo {
    stamp: FileStamp,
    info: ObjectInfo,
}

pub struct Storage {
    root: String,
    /// `list_objects` results, valid while the file's stamp is unchanged.
    /// Signature verification is the expensive part of a listing — a full
    /// read plus crypto per object, repeated every refresh tick by the tray —
    /// so it is only redone when the file actually changes. The friendly-name
    /// sidecar is deliberately *not* cached here (it is read fresh per
    /// listing): it lives outside the signed object and may be rewritten at
    /// any time by another app sharing the store.
    listing: Mutex<HashMap<ObjectId, CachedInfo>>,
}

impl Storage {
    pub fn new(root: &str) -> Self {
        Self {
            root: root.to_string(),
            listing: Mutex::new(HashMap::new()),
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
        let mut seen: HashSet<ObjectId> = HashSet::new();
        while let Some(entry) = entries.next_entry().await? {
            let filename = entry.file_name().to_string_lossy().to_string();
            if filename.starts_with('.') {
                continue;
            }
            let id = ObjectId::new(&filename);
            let metadata = entry.metadata().await?;
            let stamp = FileStamp::of(&metadata);
            let cached = {
                let listing = self.lock_listing();
                listing
                    .get(&id)
                    .filter(|cached| cached.stamp == stamp)
                    .map(|cached| cached.info.clone())
            };
            let mut info = match cached {
                Some(info) => info,
                None => {
                    let bytes = fs::read(entry.path()).await?;
                    // A file that fails to decode is treated as absent rather
                    // than failing the whole listing — protects concurrent
                    // writers in a shared user store from transiently breaking
                    // every other app's "my data" view.
                    let Ok(object) = bincode::deserialize::<Object>(&bytes) else {
                        continue;
                    };
                    let verified = object.verify();
                    let info = ObjectInfo {
                        id: object.id.clone(),
                        owner: object.payload.owner,
                        size: object.payload.metadata.size,
                        verified,
                        name: None,
                        object_type: object.payload.object_type,
                    };
                    let mut listing = self.lock_listing();
                    listing.insert(
                        id.clone(),
                        CachedInfo {
                            stamp,
                            info: info.clone(),
                        },
                    );
                    info
                }
            };
            info.name = self.read_name(&info.id).await;
            seen.insert(id);
            objects.push(info);
        }
        // Forget entries whose files are gone (evicted here or deleted by
        // another app sharing the store), so the cache stays bounded.
        self.lock_listing().retain(|id, _| seen.contains(id));
        Ok(objects)
    }

    fn lock_listing(&self) -> MutexGuard<'_, HashMap<ObjectId, CachedInfo>> {
        self.listing.lock().unwrap_or_else(|p| p.into_inner())
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
