use crate::object_id::ObjectId;
use crate::Object;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::fs;
use tokio::sync::RwLock;

/// A persisted, content-address sidecar recording which locally stored
/// objects are *cached* (fetched from another peer and re-hosted) rather
/// than owned by this node's own identity, plus the last time each cached
/// object was served so a least-recently-served eviction policy can be
/// applied safely.
///
/// This intentionally lives *outside* the signed `Object` itself: `ObjectId`
/// is `sha256(ObjectPayload)`, so recording cache metadata inside the
/// payload would change every object's identity and break compatibility with
/// already-published objects. Keeping it in a sidecar avoids that entirely
/// (see `docs/p2p-app-caching-tutorial.md` Step 3).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CacheIndex {
    /// object id hex -> unix timestamp (seconds) of the object's most recent
    /// serve. Only cached (non-owned) objects appear here.
    entries: HashMap<String, u64>,
}

impl CacheIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn load(path: &std::path::Path) -> Self {
        match fs::read(path).await {
            Ok(bytes) => match bincode::deserialize(&bytes) {
                Ok(index) => index,
                Err(e) => {
                    eprintln!("Ignoring corrupt cache index at {path:?}: {e}");
                    Self::new()
                }
            },
            Err(_) => Self::new(),
        }
    }

    pub async fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let bytes = bincode::serialize(self)?;
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, bytes).await?;
        fs::rename(tmp, path).await?;
        Ok(())
    }

    /// Records (or refreshes) that `id` is a cached object served at `now`.
    pub fn touch(&mut self, id: &ObjectId, now: u64) {
        self.entries.insert(id.0.clone(), now);
    }

    /// Removes `id` from the cache index (e.g. on eviction).
    pub fn remove(&mut self, id: &ObjectId) {
        self.entries.remove(&id.0);
    }

    /// Returns `true` if `id` is tracked as a cached object.
    pub fn is_cached(&self, id: &ObjectId) -> bool {
        self.entries.contains_key(&id.0)
    }

    /// Updates `last_served` for an object *already* tracked in the index
    /// (called when serving an object to another peer). Returns immediately
    /// without allocating if the object isn't tracked (i.e. it's an owned
    /// object that doesn't need LRU bookkeeping).
    pub fn refresh_served(&mut self, id: &ObjectId, now: u64) {
        if let Some(entry) = self.entries.get_mut(&id.0) {
            *entry = now;
        }
    }

    /// Total bytes currently held by cached objects, derived from the given
    /// `Storage` (summing the payload size of each object the index tracks).
    pub async fn total_cached_bytes(&self, storage: &super::Storage) -> u64 {
        let mut total = 0u64;
        for id in self.entries.keys() {
            let object_id = ObjectId::new(id.as_str());
            if let Ok(object) = storage.get_verified(&object_id).await {
                total += object.payload.metadata.size;
            }
        }
        total
    }

    /// Iterates cached object ids ordered by least-recently-served first,
    /// resolving each against `storage` into the object itself. Objects no
    /// longer present on disk are skipped.
    pub async fn lru_cached_objects(&self, storage: &super::Storage) -> Vec<Object> {
        let mut entries: Vec<(String, u64)> =
            self.entries.iter().map(|(k, v)| (k.clone(), *v)).collect();
        entries.sort_by_key(|(_, last_served)| *last_served);
        let mut objects = Vec::new();
        for (id, _) in entries {
            let object_id = ObjectId::new(id.as_str());
            if let Ok(object) = storage.get_verified(&object_id).await {
                objects.push(object);
            }
        }
        objects
    }
}

/// A mutable, shareable handle to the cache index plus the on-disk sidecar
/// path, held by the runtime so the CLI fetch path and the node's eviction
/// sweep can both update `last_served` and add/remove entries.
pub struct Cache {
    index: RwLock<CacheIndex>,
    path: std::path::PathBuf,
    /// The identity that "owns" objects created locally; anything with a
    /// different owner is treated as cached and eligible for eviction.
    own_identity: canopee_identity::IdentityId,
}

impl Cache {
    pub fn new(
        path: std::path::PathBuf,
        own_identity: canopee_identity::IdentityId,
        index: CacheIndex,
    ) -> Self {
        Self {
            index: RwLock::new(index),
            path,
            own_identity,
        }
    }

    /// Marks an object as cached and records its last-served time. Call this
    /// right after importing a fetched object into local storage (the CLI's
    /// `get_or_fetch` path, per tutorial Step 1). Persists the sidecar to
    /// disk.
    pub async fn mark_cached(&self, id: &ObjectId) {
        let now = time::OffsetDateTime::now_utc().unix_timestamp() as u64;
        self.index.write().await.touch(id, now);
        let _ = self.persist().await;
    }

    /// Refreshes the `last_served` timestamp for an object *already* in the
    /// cache index. Call this when serving an object to another peer (the
    /// `StorageObjectProvider::get_object` path). Does nothing for owned
    /// objects not tracked in the index, and does *not* persist to disk
    /// (the in-memory index is authoritative; persisting on every serve
    /// would be too expensive — the sidecar is flushed on `mark_cached`,
    /// `remove`, and eviction).
    pub async fn touch_served(&self, id: &ObjectId) {
        let now = time::OffsetDateTime::now_utc().unix_timestamp() as u64;
        self.index.write().await.refresh_served(id, now);
    }

    pub async fn is_cached(&self, id: &ObjectId) -> bool {
        self.index.read().await.is_cached(id)
    }

    /// Returns `true` if the object is owned by this node's own identity
    /// (and therefore must never be evicted).
    pub fn is_owned(&self, object: &Object) -> bool {
        object.payload.owner == self.own_identity
    }

    pub async fn total_cached_bytes(&self, storage: &super::Storage) -> u64 {
        self.index.read().await.total_cached_bytes(storage).await
    }

    pub async fn lru_cached_objects(&self, storage: &super::Storage) -> Vec<Object> {
        self.index.read().await.lru_cached_objects(storage).await
    }

    pub async fn remove(&self, id: &ObjectId) {
        self.index.write().await.remove(id);
        let _ = self.persist().await;
    }

    async fn persist(&self) -> Result<()> {
        let index = self.index.read().await;
        index.save(&self.path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object_id::ObjectId;
    use crate::ObjectType;
    use canopee_identity::Identity;
    use std::sync::Arc;

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("canopee_cache_test_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn touch_remove_roundtrip() {
        let mut index = CacheIndex::new();
        let id = ObjectId::new("abc");
        assert!(!index.is_cached(&id));
        index.touch(&id, 100);
        assert!(index.is_cached(&id));
        index.touch(&id, 200);
        index.remove(&id);
        assert!(!index.is_cached(&id));
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let dir = scratch_dir("roundtrip");
        let path = dir.join("cache.cache");
        let mut index = CacheIndex::new();
        index.touch(&ObjectId::new("one"), 10);
        index.touch(&ObjectId::new("two"), 20);
        index.save(&path).await.unwrap();

        let loaded = CacheIndex::load(&path).await;
        assert!(loaded.is_cached(&ObjectId::new("one")));
        assert!(loaded.is_cached(&ObjectId::new("two")));
        assert_eq!(loaded.entries.get("one"), Some(&10));
        assert_eq!(loaded.entries.get("two"), Some(&20));
    }

    #[tokio::test]
    async fn load_missing_or_corrupt_file_yields_empty_index() {
        let dir = scratch_dir("corrupt");
        let missing = CacheIndex::load(&dir.join("does-not-exist")).await;
        assert!(!missing.is_cached(&ObjectId::new("one")));

        let path = dir.join("cache.cache");
        tokio::fs::write(&path, b"not bincode").await.unwrap();
        let corrupt = CacheIndex::load(&path).await;
        assert!(!corrupt.is_cached(&ObjectId::new("one")));
    }

    #[tokio::test]
    async fn lru_orders_least_recently_served_first_and_skips_missing() {
        let dir = scratch_dir("lru");
        let storage = crate::Storage::new(dir.join("storage").to_str().unwrap());
        tokio::fs::create_dir_all(dir.join("storage")).await.unwrap();

        let owner = Arc::new(
            Identity::create(dir.join("owner.key").to_str().unwrap())
                .await
                .unwrap(),
        );
        let old = Object::new(&owner, b"old".to_vec(), ObjectType::Blob);
        let fresh = Object::new(&owner, b"fresh".to_vec(), ObjectType::Blob);
        let present_only_used_for_bytes = Object::new(&owner, b"third".to_vec(), ObjectType::Blob);
        let missing = Object::new(&owner, b"missing".to_vec(), ObjectType::Blob);
        for o in [&old, &fresh, &present_only_used_for_bytes] {
            storage.put_verified(o).await.unwrap();
        }

        let mut index = CacheIndex::new();
        index.touch(&fresh.id, 200);
        index.touch(&old.id, 100);
        index.touch(&missing.id, 50);

        let mut lru = index.lru_cached_objects(&storage).await;
        // `missing` sorts first by timestamp but is absent from storage, so it
        // must be skipped.
        assert_eq!(lru.len(), 2);
        assert_eq!(lru[0].id, old.id);
        assert_eq!(lru[1].id, fresh.id);

        // Total bytes counts every tracked object that still exists on disk,
        // including ones never on disk.
        let total = index.total_cached_bytes(&storage).await;
        assert_eq!(total, old.payload.metadata.size + fresh.payload.metadata.size);
        lru.clear();
    }

    #[tokio::test]
    async fn refresh_served_only_touches_tracked_entries() {
        let mut index = CacheIndex::new();
        let tracked = ObjectId::new("tracked");
        index.touch(&tracked, 100);
        index.refresh_served(&tracked, 400);
        assert_eq!(index.entries.get("tracked"), Some(&400));

        // Untracked (owned) objects are a no-op, not inserted.
        let owned = ObjectId::new("owned");
        index.refresh_served(&owned, 700);
        assert!(!index.is_cached(&owned));
    }

    #[tokio::test]
    async fn cache_distinguishes_owned_from_cached() {
        let dir = scratch_dir("owned");
        let own_identity = Arc::new(
            Identity::create(dir.join("own.key").to_str().unwrap())
                .await
                .unwrap(),
        );
        let other_identity = Arc::new(
            Identity::create(dir.join("other.key").to_str().unwrap())
                .await
                .unwrap(),
        );

        let cache = Cache::new(dir.join("cache.cache"), own_identity.id().clone(), CacheIndex::new());
        let mine = Object::new(&own_identity, b"mine".to_vec(), ObjectType::Blob);
        let theirs = Object::new(&other_identity, b"theirs".to_vec(), ObjectType::Blob);

        assert!(cache.is_owned(&mine));
        assert!(!cache.is_owned(&theirs));
    }
}