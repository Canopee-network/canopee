use crate::Runtime;
use canopee_protocol::{SyncRootInfo, SyncRunResult};
use canopee_storage::{ObjectId, ObjectType};
use notify::{
    EventKind, RecommendedWatcher, RecursiveMode, Watcher as _, event::ModifyKind,
    recommended_watcher,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const RECORD_SYNC: &str = "sync";
const ROOT_STATE_FILE: &str = "roots.bin";
const TOMBSTONE_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1000;
const SYNC_FILE_FORMAT: u32 = 1;
const SYNC_CHUNK_MIN: usize = 256 * 1024;
const SYNC_CHUNK_MAX: usize = 4 * 1024 * 1024;
const SYNC_CHUNK_MASK: u64 = (1 << 20) - 1;
const SYNC_CHUNK_BUFFER: usize = 64 * 1024;
const SYNC_CHUNK_SEED: u64 = 0x6a09_e667_f3bc_c909;

#[derive(Debug)]
struct SyncIndexChanged;

impl std::fmt::Display for SyncIndexChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("sync root index changed while syncing")
    }
}

impl std::error::Error for SyncIndexChanged {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct SyncRootIndex {
    version: u64,
    roots: Vec<SyncRootRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SyncRootRecord {
    name: String,
    root_id: String,
    manifest: ObjectId,
    revision: u64,
    updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SyncManifest {
    root_id: String,
    revision: u64,
    entries: Vec<SyncManifestEntry>,
    /// Paths currently deleted in this root. Kept indefinitely for now so a
    /// stale device cannot resurrect them by re-uploading an old file.
    #[serde(default)]
    tombstones: Vec<SyncTombstone>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SyncManifestEntry {
    path: String,
    object: ObjectId,
    size: u64,
    content_hash: [u8; 32],
    modified_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SyncFileIndex {
    format: u32,
    size: u64,
    content_hash: [u8; 32],
    chunks: Vec<SyncFileChunk>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SyncFileChunk {
    object: ObjectId,
    size: u64,
    content_hash: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SyncTombstone {
    path: String,
    deleted_at: u64,
    /// Content the deleting device believed it was deleting. This distinguishes
    /// a normal propagate-delete from a stale delete racing a local edit.
    /// Older manifests deserialize this as `None`.
    #[serde(default)]
    content_hash: Option<[u8; 32]>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct LocalSyncState {
    roots: Vec<LocalSyncRoot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LocalSyncRoot {
    name: String,
    root_id: String,
    path: PathBuf,
    base_manifest: Option<ObjectId>,
    base_revision: u64,
    last_sync_at: Option<u64>,
}

#[derive(Clone, Debug)]
struct ScannedFile {
    content_hash: [u8; 32],
}

struct SyncFileChunker<R> {
    reader: R,
    buffer: Vec<u8>,
    position: usize,
    filled: usize,
}

impl<R: tokio::io::AsyncRead + Unpin> SyncFileChunker<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: vec![0; SYNC_CHUNK_BUFFER],
            position: 0,
            filled: 0,
        }
    }

    async fn next_chunk(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        let mut chunk = Vec::with_capacity(SYNC_CHUNK_MIN);
        let mut hash = 0u64;
        loop {
            if self.position == self.filled {
                self.filled = self.reader.read(&mut self.buffer).await?;
                self.position = 0;
                if self.filled == 0 {
                    break;
                }
            }
            let byte = self.buffer[self.position];
            self.position += 1;
            chunk.push(byte);
            hash = gear_hash_step(hash, byte);
            if chunk.len() >= SYNC_CHUNK_MIN
                && ((hash & SYNC_CHUNK_MASK) == 0 || chunk.len() >= SYNC_CHUNK_MAX)
            {
                break;
            }
        }
        if chunk.is_empty() {
            Ok(None)
        } else {
            Ok(Some(chunk))
        }
    }
}

fn gear_hash_step(hash: u64, byte: u8) -> u64 {
    (hash << 1).wrapping_add(gear_table()[byte as usize])
}

fn gear_table() -> &'static [u64; 256] {
    static TABLE: OnceLock<[u64; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut state = SYNC_CHUNK_SEED;
        let mut table = [0u64; 256];
        for slot in &mut table {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut value = state;
            value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            *slot = value ^ (value >> 31);
        }
        table
    })
}

impl Runtime {
    pub(crate) const PERIODIC_FOLDER_SYNC_INTERVAL: std::time::Duration =
        std::time::Duration::from_secs(30);
    const FOLDER_WATCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);
    const FOLDER_WATCH_RECONCILE: std::time::Duration = std::time::Duration::from_secs(5);

    pub(crate) fn spawn_periodic_folder_sync(&self) {
        let runtime = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Self::PERIODIC_FOLDER_SYNC_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if !runtime.state.read().await.started {
                    continue;
                }
                let peer_count = runtime.network.peers().await.map(|p| p.len()).unwrap_or(0);
                if peer_count == 0 {
                    continue;
                }
                match runtime.sync_now(None).await {
                    Ok(results) if results.iter().any(SyncRunResult::changed) => {
                        tracing::info!("periodic folder sync: {results:?}");
                    }
                    Ok(_) => {}
                    Err(e) => tracing::debug!("periodic folder sync skipped: {e}"),
                }
            }
        });
    }

    pub(crate) fn spawn_folder_watcher(&self) {
        let runtime = self.clone();
        tokio::spawn(async move {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(1);
            let mut watchers: BTreeMap<PathBuf, RecommendedWatcher> = BTreeMap::new();
            let mut reconcile = tokio::time::interval(Self::FOLDER_WATCH_RECONCILE);
            reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = reconcile.tick() => {
                        runtime.reconcile_folder_watchers(&tx, &mut watchers).await;
                    }
                    event = rx.recv() => {
                        if event.is_none() {
                            return;
                        }
                        loop {
                            match tokio::time::timeout(
                                Self::FOLDER_WATCH_DEBOUNCE,
                                rx.recv(),
                            )
                            .await
                            {
                                Ok(Some(())) => {}
                                Ok(None) => return,
                                Err(_) => break,
                            }
                        }
                        runtime.run_watcher_sync().await;
                    }
                }
            }
        });
    }

    async fn reconcile_folder_watchers(
        &self,
        tx: &tokio::sync::mpsc::Sender<()>,
        watchers: &mut BTreeMap<PathBuf, RecommendedWatcher>,
    ) {
        let paths: BTreeSet<PathBuf> = match self.load_sync_state().await {
            Ok(state) => state.roots.into_iter().map(|root| root.path).collect(),
            Err(e) => {
                tracing::debug!("folder watcher could not load sync roots: {e}");
                return;
            }
        };
        watchers.retain(|path, _| paths.contains(path));
        for path in paths {
            if watchers.contains_key(&path) {
                continue;
            }
            let tx = tx.clone();
            let mut watcher =
                match recommended_watcher(move |result: notify::Result<notify::Event>| {
                    if let Ok(event) = result {
                        if watcher_event_matters(&event) {
                            let _ = tx.try_send(());
                        }
                    }
                }) {
                    Ok(watcher) => watcher,
                    Err(e) => {
                        tracing::debug!("folder watcher for {} failed: {e}", path.display());
                        continue;
                    }
                };
            if let Err(e) = watcher.watch(&path, RecursiveMode::Recursive) {
                tracing::debug!("folder watcher could not watch {}: {e}", path.display());
                continue;
            }
            watchers.insert(path, watcher);
        }
    }

    async fn run_watcher_sync(&self) {
        if !self.state.read().await.started {
            return;
        }
        let peer_count = self.network.peers().await.map(|p| p.len()).unwrap_or(0);
        if peer_count == 0 {
            return;
        }
        match self.sync_now(None).await {
            Ok(results) if results.iter().any(SyncRunResult::changed) => {
                tracing::info!("watcher folder sync: {results:?}");
            }
            Ok(_) => {}
            Err(e) => tracing::debug!("watcher folder sync skipped: {e}"),
        }
    }

    pub async fn sync_create_root(
        &self,
        path: &str,
        name: Option<&str>,
    ) -> anyhow::Result<SyncRootInfo> {
        let _guard = self.folder_sync_lock.lock().await;
        let path = ensure_directory(path).await?;
        let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(name) => name.to_string(),
            None => path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("cannot derive a sync root name from {path:?}"))?,
        };
        validate_root_name(&name)?;

        let mut index = self.refresh_sync_index().await?;
        if index.roots.iter().any(|r| r.name == name) {
            anyhow::bail!("a sync root named \"{name}\" already exists");
        }
        let mut state = self.load_sync_state().await?;
        if state.roots.iter().any(|r| r.name == name) {
            anyhow::bail!("a local sync root named \"{name}\" already exists");
        }
        if state.roots.iter().any(|r| r.path == path) {
            anyhow::bail!("{} is already mapped to a sync root", path.display());
        }

        let root_id = new_root_id();
        let mut entries = Vec::new();
        for relative in scan_directory(&path)?.into_keys() {
            entries.push(self.upload_file_entry(&path, &relative, None).await?);
        }

        let now = unix_millis();
        let manifest = SyncManifest {
            root_id: root_id.clone(),
            revision: 1,
            entries,
            tombstones: Vec::new(),
        };
        let manifest_id = self.store_sync_manifest(&manifest).await?;
        index.roots.push(SyncRootRecord {
            name: name.clone(),
            root_id: root_id.clone(),
            manifest: manifest_id.clone(),
            revision: 1,
            updated_at: now,
        });
        self.publish_sync_index(&mut index).await?;

        let local = LocalSyncRoot {
            name: name.clone(),
            root_id: root_id.clone(),
            path: path.clone(),
            base_manifest: Some(manifest_id.clone()),
            base_revision: 1,
            last_sync_at: Some(now),
        };
        state.roots.push(local.clone());
        self.save_sync_state(&state).await?;

        Ok(SyncRootInfo {
            name,
            root_id,
            path: Some(path.display().to_string()),
            revision: 1,
            manifest: Some(manifest_id),
            last_sync_at: Some(now),
        })
    }

    pub async fn sync_join_root(&self, name: &str, path: &str) -> anyhow::Result<SyncRootInfo> {
        let _guard = self.folder_sync_lock.lock().await;
        validate_root_name(name)?;
        let path = ensure_directory(path).await?;
        let index = self.refresh_sync_index().await?;
        let root = index
            .roots
            .iter()
            .find(|r| r.name == name)
            .ok_or_else(|| anyhow::anyhow!("no account sync root named \"{name}\""))?;

        let mut state = self.load_sync_state().await?;
        if state.roots.iter().any(|r| r.name == name) {
            anyhow::bail!("sync root \"{name}\" is already joined on this device");
        }
        if state.roots.iter().any(|r| r.path == path) {
            anyhow::bail!("{} is already mapped to a sync root", path.display());
        }

        let local = LocalSyncRoot {
            name: name.to_string(),
            root_id: root.root_id.clone(),
            path: path.clone(),
            base_manifest: None,
            base_revision: 0,
            last_sync_at: None,
        };
        state.roots.push(local);
        self.save_sync_state(&state).await?;

        Ok(SyncRootInfo {
            name: name.to_string(),
            root_id: root.root_id.clone(),
            path: Some(path.display().to_string()),
            revision: root.revision,
            manifest: Some(root.manifest.clone()),
            last_sync_at: None,
        })
    }

    pub async fn sync_list_roots(&self) -> anyhow::Result<Vec<SyncRootInfo>> {
        let state = self.load_sync_state().await?;
        let index = self.load_sync_index().await?;
        let mut infos = Vec::new();
        for record in &index.roots {
            let local = state.roots.iter().find(|r| r.name == record.name);
            infos.push(root_info(Some(record), local, &record.name));
        }
        for local in &state.roots {
            if !index.roots.iter().any(|r| r.name == local.name) {
                infos.push(root_info(None, Some(local), &local.name));
            }
        }
        infos.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(infos)
    }

    pub async fn sync_status(&self, name: Option<&str>) -> anyhow::Result<Vec<SyncRootInfo>> {
        let roots = self.sync_list_roots().await?;
        match name {
            Some(name) => {
                let root = roots
                    .into_iter()
                    .find(|r| r.name == name)
                    .ok_or_else(|| anyhow::anyhow!("no sync root named \"{name}\""))?;
                Ok(vec![root])
            }
            None => Ok(roots),
        }
    }

    pub async fn sync_now(&self, name: Option<&str>) -> anyhow::Result<Vec<SyncRunResult>> {
        let _guard = self.folder_sync_lock.lock().await;
        let mut state = self.load_sync_state().await?;
        if state.roots.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(name) = name {
            if !state.roots.iter().any(|r| r.name == name) {
                anyhow::bail!("no local sync root named \"{name}\"");
            }
        }
        let selected: Vec<String> = state
            .roots
            .iter()
            .filter(|r| name.is_none_or(|n| r.name == n))
            .map(|r| r.name.clone())
            .collect();

        let mut index = self.refresh_sync_index().await?;
        let mut results = Vec::new();
        for root_name in selected {
            let mut attempts = 0;
            loop {
                let local = state
                    .roots
                    .iter_mut()
                    .find(|r| r.name == root_name)
                    .expect("selected sync root must still exist");
                match self.sync_one_root(&mut index, local).await {
                    Ok(result) => {
                        results.push(result);
                        self.save_sync_state(&state).await?;
                        break;
                    }
                    Err(e) if e.downcast_ref::<SyncIndexChanged>().is_some() => {
                        attempts += 1;
                        if attempts >= 5 {
                            return Err(anyhow::anyhow!(
                                "sync root index kept changing while syncing {root_name}; retry"
                            ));
                        }
                        index = self.refresh_sync_index().await?;
                    }
                    Err(e) => {
                        let _ = self.save_sync_state(&state).await;
                        return Err(e);
                    }
                }
            }
        }
        Ok(results)
    }

    pub async fn sync_remove_root(&self, name: &str) -> anyhow::Result<()> {
        let _guard = self.folder_sync_lock.lock().await;
        let mut state = self.load_sync_state().await?;
        let before = state.roots.len();
        state.roots.retain(|r| r.name != name);
        if state.roots.len() == before {
            anyhow::bail!("no local sync root named \"{name}\"");
        }
        self.save_sync_state(&state).await
    }

    async fn sync_one_root(
        &self,
        index: &mut SyncRootIndex,
        local: &mut LocalSyncRoot,
    ) -> anyhow::Result<SyncRunResult> {
        let record = index
            .roots
            .iter()
            .find(|r| r.name == local.name)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "sync root \"{}\" is no longer in the account index",
                    local.name
                )
            })?;
        if record.root_id != local.root_id {
            anyhow::bail!(
                "sync root \"{}\" was recreated remotely (root id changed); remove and join it again",
                local.name
            );
        }

        tokio::fs::create_dir_all(&local.path).await?;
        if !tokio::fs::metadata(&local.path).await?.is_dir() {
            anyhow::bail!("sync root path {} is not a directory", local.path.display());
        }

        let base = match &local.base_manifest {
            Some(id) => Some(self.load_sync_manifest(id).await?),
            None => None,
        };
        let base_entries: BTreeMap<String, SyncManifestEntry> = base
            .as_ref()
            .map(|m| entry_map(&m.entries))
            .unwrap_or_default();
        let base_tombstones: BTreeMap<String, SyncTombstone> = base
            .as_ref()
            .map(|m| tombstone_map(&m.tombstones))
            .unwrap_or_default();
        let remote = self.load_sync_manifest(&record.manifest).await?;
        if remote.root_id != record.root_id {
            anyhow::bail!("sync manifest root id does not match the account index");
        }
        let remote_entries = entry_map(&remote.entries);
        let remote_tombstones = tombstone_map(&remote.tombstones);
        let local_files = scan_directory(&local.path)?;

        let mut local_changed = BTreeSet::new();
        for (path, file) in &local_files {
            if base_entries.get(path).map(|entry| entry.content_hash) != Some(file.content_hash) {
                local_changed.insert(path.clone());
            }
        }
        for path in base_entries.keys() {
            if !local_files.contains_key(path) {
                local_changed.insert(path.clone());
            }
        }

        let mut remote_changed = BTreeSet::new();
        for (path, entry) in &remote_entries {
            if base_entries.get(path).map(|e| e.content_hash) != Some(entry.content_hash) {
                remote_changed.insert(path.clone());
            }
        }
        for path in base_entries.keys() {
            if !remote_entries.contains_key(path) && !remote_tombstones.contains_key(path) {
                remote_changed.insert(path.clone());
            }
        }
        for path in remote_tombstones.keys() {
            if !base_tombstones.contains_key(path) {
                remote_changed.insert(path.clone());
            }
        }
        for path in base_tombstones.keys() {
            if !remote_tombstones.contains_key(path) {
                remote_changed.insert(path.clone());
            }
        }

        let now = unix_millis();
        let mut merged_entries = remote_entries.clone();
        let mut merged_tombstones = remote_tombstones.clone();
        let mut result = SyncRunResult {
            root: local.name.clone(),
            ..SyncRunResult::default()
        };

        self.ensure_sync_record_unchanged(&record).await?;

        for path in &remote_changed {
            let local_file = local_files.get(path);
            let remote_entry = remote_entries.get(path);
            let remote_tombstone = remote_tombstones.get(path);
            match (remote_entry, remote_tombstone) {
                (Some(entry), None) => {
                    if let Some(local_file) = local_file {
                        if local_changed.contains(path)
                            && local_file.content_hash != entry.content_hash
                        {
                            let conflict = self.write_conflict_copy(&local.path, path).await?;
                            let conflict_entry =
                                self.upload_file_entry(&local.path, &conflict, None).await?;
                            merged_entries.insert(conflict, conflict_entry);
                            result.conflicts += 1;
                            result.uploaded += 1;
                        }
                    }
                    let already_matches = local_file
                        .map(|file| file.content_hash == entry.content_hash)
                        .unwrap_or(false);
                    if !already_matches {
                        self.write_remote_file(&local.path, entry).await?;
                        result.downloaded += 1;
                    }
                }
                (None, _) => {
                    if local_file.is_some() && !local_changed.contains(path) {
                        if remote_tombstone_covers_local(remote_tombstone, base_entries.get(path)) {
                            remove_local_file(&local.path, path).await?;
                            result.removed += 1;
                        } else {
                            local_changed.insert(path.clone());
                        }
                    }
                }
                (Some(_), Some(_)) => {
                    unreachable!("manifest validation rejects file+tombstone")
                }
            }
        }

        for path in &local_changed {
            let remote_wins = remote_changed.contains(path) && remote_entries.contains_key(path);
            if remote_wins {
                continue;
            }
            match local_files.get(path) {
                Some(file) => {
                    let already_matches = merged_entries
                        .get(path)
                        .map(|entry| entry.content_hash == file.content_hash)
                        .unwrap_or(false);
                    if already_matches {
                        merged_tombstones.remove(path);
                        continue;
                    }
                    let entry = self
                        .upload_file_entry(&local.path, path, base_entries.get(path))
                        .await?;
                    if entry.content_hash != file.content_hash {
                        anyhow::bail!("{path} changed while it was being synced; retry");
                    }
                    merged_tombstones.remove(path);
                    merged_entries.insert(path.clone(), entry);
                    result.uploaded += 1;
                }
                None => {
                    merged_entries.remove(path);
                    merged_tombstones
                        .entry(path.clone())
                        .or_insert_with(|| SyncTombstone {
                            path: path.clone(),
                            deleted_at: now,
                            content_hash: base_entries.get(path).map(|entry| entry.content_hash),
                        });
                }
            }
        }

        self.ensure_sync_record_unchanged(&record).await?;

        let now = unix_millis();
        prune_tombstones(&mut merged_tombstones, now);
        let merged_changed =
            merged_entries != remote_entries || merged_tombstones != remote_tombstones;
        let (manifest_id, revision) = if merged_changed {
            let revision = record.revision.max(local.base_revision).saturating_add(1);
            let manifest = SyncManifest {
                root_id: record.root_id.clone(),
                revision,
                entries: merged_entries.into_values().collect(),
                tombstones: merged_tombstones.into_values().collect(),
            };
            let manifest_id = self.store_sync_manifest(&manifest).await?;
            if let Some(record) = index.roots.iter_mut().find(|r| r.name == local.name) {
                record.manifest = manifest_id.clone();
                record.revision = revision;
                record.updated_at = now;
            }
            self.publish_sync_index(index).await?;
            (manifest_id, revision)
        } else {
            (record.manifest.clone(), record.revision)
        };

        local.base_manifest = Some(manifest_id);
        local.base_revision = revision;
        local.last_sync_at = Some(now);
        Ok(result)
    }

    async fn ensure_sync_record_unchanged(&self, record: &SyncRootRecord) -> anyhow::Result<()> {
        self.sync_record(RECORD_SYNC).await?;
        let index = self.load_sync_index().await?;
        match index.roots.iter().find(|root| root.name == record.name) {
            Some(current) if current.manifest == record.manifest => Ok(()),
            _ => Err(SyncIndexChanged.into()),
        }
    }

    async fn refresh_sync_index(&self) -> anyhow::Result<SyncRootIndex> {
        self.sync_record(RECORD_SYNC).await?;
        self.load_sync_index().await
    }

    async fn load_sync_index(&self) -> anyhow::Result<SyncRootIndex> {
        let Some(record) = self
            .resolve_pointer(self.identity.id(), RECORD_SYNC)
            .await?
        else {
            return Ok(SyncRootIndex::default());
        };
        let object = self.get(&record.manifest).await?;
        object.decode()
    }

    async fn publish_sync_index(&self, index: &mut SyncRootIndex) -> anyhow::Result<()> {
        index.version = index.version.saturating_add(1);
        let object = self
            .put_object(bincode::serialize(index)?, ObjectType::Blob, None)
            .await?;
        self.mark_shared(&object.id).await;
        self.publish_pointer(RECORD_SYNC, object.id).await
    }

    async fn store_sync_manifest(&self, manifest: &SyncManifest) -> anyhow::Result<ObjectId> {
        validate_manifest(manifest)?;
        let object = self
            .put_object(bincode::serialize(manifest)?, ObjectType::Blob, None)
            .await?;
        self.mark_shared(&object.id).await;
        Ok(object.id)
    }

    async fn load_sync_manifest(&self, id: &ObjectId) -> anyhow::Result<SyncManifest> {
        let object = self.get(id).await?;
        let manifest: SyncManifest = object.decode()?;
        validate_manifest(&manifest)?;
        Ok(manifest)
    }

    /// Re-marks locally owned sync objects as servable after a restart. The
    /// shared set is intentionally in-memory, so without this a node restart
    /// would leave its own encrypted manifests/file objects unreachable to the
    /// other devices until the next local edit re-uploaded them.
    pub(crate) async fn reconcile_sync_shared(&self) {
        let state = match self.load_sync_state().await {
            Ok(state) if !state.roots.is_empty() => state,
            _ => return,
        };
        if let Ok(Some(record)) = self.resolve_pointer(self.identity.id(), RECORD_SYNC).await {
            self.mark_shared(&record.manifest).await;
            if let Ok(index) = self.load_sync_index().await {
                for local in &state.roots {
                    if let Some(root) = index.roots.iter().find(|r| r.name == local.name) {
                        self.mark_shared(&root.manifest).await;
                        if let Ok(manifest) = self.load_sync_manifest(&root.manifest).await {
                            for entry in &manifest.entries {
                                self.mark_sync_entry_shared(entry).await;
                            }
                        }
                    }
                }
            }
        }
        for local in &state.roots {
            if let Some(id) = &local.base_manifest {
                self.mark_shared(id).await;
                if let Ok(manifest) = self.load_sync_manifest(id).await {
                    for entry in &manifest.entries {
                        self.mark_sync_entry_shared(entry).await;
                    }
                }
            }
        }
    }

    async fn mark_sync_entry_shared(&self, entry: &SyncManifestEntry) {
        self.mark_shared(&entry.object).await;
        let Ok(object) = self.storage.get_verified(&entry.object).await else {
            return;
        };
        if object.object_type() != ObjectType::SyncFile {
            return;
        }
        let Ok(opened) = self.open_object(&object) else {
            return;
        };
        let Ok(index) = opened.decode::<SyncFileIndex>() else {
            return;
        };
        for chunk in index.chunks {
            self.mark_shared(&chunk.object).await;
        }
    }

    async fn write_remote_file(
        &self,
        root: &Path,
        entry: &SyncManifestEntry,
    ) -> anyhow::Result<()> {
        let object = self.get(&entry.object).await?;
        if object.object_type() == ObjectType::SyncFile {
            let index: SyncFileIndex = object.decode()?;
            validate_sync_file_index(&index, entry)?;
            return self.write_remote_chunked_file(root, entry, &index).await;
        }
        let data = object.payload.data;
        let actual: [u8; 32] = Sha256::digest(&data).into();
        if actual != entry.content_hash {
            anyhow::bail!(
                "sync manifest hash mismatch for {} ({})",
                entry.path,
                entry.object
            );
        }
        self.write_remote_bytes(root, entry, &data).await
    }

    async fn write_remote_chunked_file(
        &self,
        root: &Path,
        entry: &SyncManifestEntry,
        index: &SyncFileIndex,
    ) -> anyhow::Result<()> {
        let target = join_relative(root, &entry.path);
        let parent = target
            .parent()
            .ok_or_else(|| anyhow::anyhow!("sync target has no parent directory"))?;
        tokio::fs::create_dir_all(parent).await?;
        let tmp = target.with_file_name(format!(
            ".canopee-sync-tmp-{}-{}",
            std::process::id(),
            entry.object
        ));
        let write = async {
            let mut file = tokio::fs::File::create(&tmp).await?;
            let mut whole = Sha256::new();
            let mut total = 0u64;
            for chunk in &index.chunks {
                let object = self.get(&chunk.object).await?;
                if object.object_type() != ObjectType::Blob {
                    anyhow::bail!("sync chunk {} is not a blob", chunk.object);
                }
                let data = object.payload.data;
                if data.len() as u64 != chunk.size {
                    anyhow::bail!("sync chunk {} has the wrong size", chunk.object);
                }
                let actual: [u8; 32] = Sha256::digest(&data).into();
                if actual != chunk.content_hash {
                    anyhow::bail!("sync chunk {} has the wrong hash", chunk.object);
                }
                whole.update(&data);
                total = total
                    .checked_add(chunk.size)
                    .ok_or_else(|| anyhow::anyhow!("sync file size overflow"))?;
                file.write_all(&data).await?;
            }
            if total != entry.size {
                anyhow::bail!("sync file {} has the wrong size", entry.path);
            }
            let actual: [u8; 32] = whole.finalize().into();
            if actual != entry.content_hash {
                anyhow::bail!("sync manifest hash mismatch for {}", entry.path);
            }
            file.sync_all().await?;
            drop(file);
            tokio::fs::rename(&tmp, &target).await?;
            #[cfg(unix)]
            if let Ok(directory) = tokio::fs::File::open(parent).await {
                let _ = directory.sync_all().await;
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(e) = write {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
        Ok(())
    }

    async fn write_remote_bytes(
        &self,
        root: &Path,
        entry: &SyncManifestEntry,
        data: &[u8],
    ) -> anyhow::Result<()> {
        let target = join_relative(root, &entry.path);
        let parent = target
            .parent()
            .ok_or_else(|| anyhow::anyhow!("sync target has no parent directory"))?;
        tokio::fs::create_dir_all(parent).await?;
        let tmp = target.with_file_name(format!(
            ".canopee-sync-tmp-{}-{}",
            std::process::id(),
            entry.object
        ));
        let write = async {
            let mut file = tokio::fs::File::create(&tmp).await?;
            file.write_all(data).await?;
            file.sync_all().await?;
            drop(file);
            tokio::fs::rename(&tmp, &target).await?;
            #[cfg(unix)]
            if let Ok(directory) = tokio::fs::File::open(parent).await {
                let _ = directory.sync_all().await;
            }
            Ok::<(), std::io::Error>(())
        }
        .await;
        if let Err(e) = write {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e.into());
        }
        Ok(())
    }

    async fn upload_file_entry(
        &self,
        root: &Path,
        relative: &str,
        previous: Option<&SyncManifestEntry>,
    ) -> anyhow::Result<SyncManifestEntry> {
        let path = join_relative(root, relative);
        let modified_at = tokio::fs::metadata(&path)
            .await
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let mut reusable = self.reusable_chunks(previous).await;
        let mut chunker = SyncFileChunker::new(tokio::fs::File::open(&path).await?);
        let mut chunks = Vec::new();
        let mut whole = Sha256::new();
        let mut size = 0u64;
        while let Some(data) = chunker.next_chunk().await? {
            let chunk_size = data.len() as u64;
            size = size
                .checked_add(chunk_size)
                .ok_or_else(|| anyhow::anyhow!("sync file size overflow"))?;
            let content_hash: [u8; 32] = Sha256::digest(&data).into();
            whole.update(&data);
            let key = (content_hash, chunk_size);
            let object = if let Some(id) = reusable.get(&key).cloned() {
                self.mark_shared(&id).await;
                id
            } else {
                let object = self.put_object(data, ObjectType::Blob, None).await?;
                self.mark_shared(&object.id).await;
                reusable.insert(key, object.id.clone());
                object.id
            };
            chunks.push(SyncFileChunk {
                object,
                size: chunk_size,
                content_hash,
            });
        }
        let content_hash: [u8; 32] = whole.finalize().into();
        let index = SyncFileIndex {
            format: SYNC_FILE_FORMAT,
            size,
            content_hash,
            chunks,
        };
        let object = self
            .put_object(bincode::serialize(&index)?, ObjectType::SyncFile, None)
            .await?;
        self.mark_shared(&object.id).await;
        Ok(SyncManifestEntry {
            path: relative.to_string(),
            object: object.id,
            size,
            content_hash,
            modified_at,
        })
    }

    async fn reusable_chunks(
        &self,
        previous: Option<&SyncManifestEntry>,
    ) -> HashMap<([u8; 32], u64), ObjectId> {
        let mut reusable = HashMap::new();
        let Some(previous) = previous else {
            return reusable;
        };
        let Ok(object) = self.storage.get_verified(&previous.object).await else {
            return reusable;
        };
        if object.object_type() != ObjectType::SyncFile {
            return reusable;
        }
        let Ok(opened) = self.open_object(&object) else {
            return reusable;
        };
        let Ok(index) = opened.decode::<SyncFileIndex>() else {
            return reusable;
        };
        if index.format != SYNC_FILE_FORMAT {
            return reusable;
        }
        for chunk in index.chunks {
            if self.storage.get_verified(&chunk.object).await.is_ok() {
                reusable
                    .entry((chunk.content_hash, chunk.size))
                    .or_insert(chunk.object);
            }
        }
        reusable
    }

    async fn write_conflict_copy(&self, root: &Path, relative: &str) -> anyhow::Result<String> {
        let source = join_relative(root, relative);
        let file_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("conflict source has no file name"))?;
        let mut stamp = unix_millis();
        loop {
            let conflict_name = format!("{file_name}.conflict-{stamp}");
            let candidate = source.with_file_name(&conflict_name);
            if !tokio::fs::try_exists(&candidate).await? {
                tokio::fs::copy(&source, &candidate).await?;
                let conflict_relative = relative
                    .rsplit_once('/')
                    .map(|(parent, _)| format!("{parent}/{conflict_name}"))
                    .unwrap_or(conflict_name);
                validate_relative_path(&conflict_relative)?;
                return Ok(conflict_relative);
            }
            stamp = stamp.saturating_add(1);
        }
    }

    async fn load_sync_state(&self) -> anyhow::Result<LocalSyncState> {
        let path = self.sync_state_path();
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(bincode::deserialize(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LocalSyncState::default()),
            Err(e) => Err(e.into()),
        }
    }

    async fn save_sync_state(&self, state: &LocalSyncState) -> anyhow::Result<()> {
        let path = self.sync_state_path();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, bincode::serialize(state)?).await?;
        tokio::fs::rename(tmp, path).await?;
        Ok(())
    }

    fn sync_state_path(&self) -> PathBuf {
        self.config.sync_path().join(ROOT_STATE_FILE)
    }
}

fn validate_sync_file_index(
    index: &SyncFileIndex,
    entry: &SyncManifestEntry,
) -> anyhow::Result<()> {
    if index.format != SYNC_FILE_FORMAT {
        anyhow::bail!("unsupported sync file format {}", index.format);
    }
    if index.size != entry.size {
        anyhow::bail!("sync file index size does not match the manifest entry");
    }
    if index.content_hash != entry.content_hash {
        anyhow::bail!("sync file index hash does not match the manifest entry");
    }
    let mut total = 0u64;
    for chunk in &index.chunks {
        if chunk.size == 0 || chunk.size > SYNC_CHUNK_MAX as u64 {
            anyhow::bail!("sync file index contains an invalid chunk size");
        }
        total = total
            .checked_add(chunk.size)
            .ok_or_else(|| anyhow::anyhow!("sync file size overflow"))?;
    }
    if total != index.size {
        anyhow::bail!("sync file index chunk sizes do not add up");
    }
    Ok(())
}

fn validate_manifest(manifest: &SyncManifest) -> anyhow::Result<()> {
    let mut paths = BTreeSet::new();
    for entry in &manifest.entries {
        validate_relative_path(&entry.path)?;
        if !paths.insert(entry.path.as_str()) {
            anyhow::bail!("sync manifest contains duplicate path {:?}", entry.path);
        }
    }
    for tombstone in &manifest.tombstones {
        validate_relative_path(&tombstone.path)?;
        if !paths.insert(tombstone.path.as_str()) {
            anyhow::bail!(
                "sync manifest contains duplicate file/tombstone path {:?}",
                tombstone.path
            );
        }
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> anyhow::Result<()> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains('\0') {
        anyhow::bail!("invalid sync path {path:?}");
    }
    for component in path.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.chars().any(char::is_control)
        {
            anyhow::bail!("invalid sync path {path:?}");
        }
    }
    Ok(())
}

fn entry_map(entries: &[SyncManifestEntry]) -> BTreeMap<String, SyncManifestEntry> {
    entries
        .iter()
        .cloned()
        .map(|entry| (entry.path.clone(), entry))
        .collect()
}

fn watcher_event_matters(event: &notify::Event) -> bool {
    let kind_matters = matches!(
        event.kind,
        EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Name(_))
    );
    kind_matters && event.paths.iter().all(|path| !is_sync_temp_path(path))
}

fn is_sync_temp_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".canopee-sync-tmp-"))
}

fn prune_tombstones(tombstones: &mut BTreeMap<String, SyncTombstone>, now: u64) {
    tombstones
        .retain(|_, tombstone| now.saturating_sub(tombstone.deleted_at) <= TOMBSTONE_RETENTION_MS);
}

fn remote_tombstone_covers_local(
    tombstone: Option<&SyncTombstone>,
    base: Option<&SyncManifestEntry>,
) -> bool {
    match tombstone.and_then(|tombstone| tombstone.content_hash) {
        Some(hash) => base
            .map(|entry| entry.content_hash == hash)
            .unwrap_or(false),
        None => true,
    }
}

fn tombstone_map(tombstones: &[SyncTombstone]) -> BTreeMap<String, SyncTombstone> {
    tombstones
        .iter()
        .cloned()
        .map(|tombstone| (tombstone.path.clone(), tombstone))
        .collect()
}

fn root_info(
    record: Option<&SyncRootRecord>,
    local: Option<&LocalSyncRoot>,
    name: &str,
) -> SyncRootInfo {
    SyncRootInfo {
        name: name.to_string(),
        root_id: record
            .map(|r| r.root_id.clone())
            .or_else(|| local.map(|l| l.root_id.clone()))
            .unwrap_or_default(),
        path: local.map(|l| l.path.display().to_string()),
        revision: record
            .map(|r| r.revision)
            .or_else(|| local.map(|l| l.base_revision))
            .unwrap_or(0),
        manifest: record
            .map(|r| r.manifest.clone())
            .or_else(|| local.and_then(|l| l.base_manifest.clone())),
        last_sync_at: local.and_then(|l| l.last_sync_at),
    }
}

fn validate_root_name(name: &str) -> anyhow::Result<()> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("sync root name must not be empty");
    }
    if name == "." || name == ".." {
        anyhow::bail!("sync root name \"{name}\" is invalid");
    }
    if name
        .chars()
        .any(|c| c == '/' || c == '\\' || c == '\0' || c.is_control())
    {
        anyhow::bail!("sync root name \"{name}\" contains invalid characters");
    }
    Ok(())
}

fn new_root_id() -> String {
    let stamp = unix_millis();
    let nonce = canopee_identity::pairing::generate_session_id();
    format!("{stamp:x}-{nonce}")
}

async fn ensure_directory(path: &str) -> anyhow::Result<PathBuf> {
    let path = PathBuf::from(path);
    tokio::fs::create_dir_all(&path).await?;
    let canonical = tokio::fs::canonicalize(&path).await?;
    if !tokio::fs::metadata(&canonical).await?.is_dir() {
        anyhow::bail!("{} is not a directory", canonical.display());
    }
    Ok(canonical)
}

fn unix_millis() -> u64 {
    (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as u64
}

fn scan_directory(root: &Path) -> anyhow::Result<BTreeMap<String, ScannedFile>> {
    fn visit(
        root: &Path,
        directory: &Path,
        files: &mut BTreeMap<String, ScannedFile>,
    ) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                visit(root, &path, files)?;
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with(".canopee-sync-tmp-") {
                continue;
            }
            let relative = path
                .strip_prefix(root)?
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            validate_relative_path(&relative)?;
            files.insert(
                relative,
                ScannedFile {
                    content_hash: hash_file(&path)?,
                },
            );
        }
        Ok(())
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files)
}

fn hash_file(path: &Path) -> anyhow::Result<[u8; 32]> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; SYNC_CHUNK_BUFFER];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

fn join_relative(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for component in relative.split('/') {
        path.push(component);
    }
    path
}

async fn remove_local_file(root: &Path, relative: &str) -> anyhow::Result<()> {
    let path = join_relative(root, relative);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_paths_cannot_escape_the_root() {
        for path in ["", "/etc/passwd", "../secret", "a/../../b", "a//b", "a/./b"] {
            assert!(
                validate_relative_path(path).is_err(),
                "{path:?} should be rejected"
            );
        }
        for path in ["hello.txt", "sub/nested.txt", "a b/c-d.txt"] {
            validate_relative_path(path).expect("valid relative path");
        }
    }

    #[test]
    fn manifest_rejects_duplicate_paths() {
        let entry = SyncManifestEntry {
            path: "same.txt".into(),
            object: ObjectId::new("object"),
            size: 1,
            content_hash: [0; 32],
            modified_at: 1,
        };
        let manifest = SyncManifest {
            root_id: "root".into(),
            revision: 1,
            entries: vec![entry.clone(), entry],
            tombstones: Vec::new(),
        };
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn manifest_rejects_file_and_tombstone_for_same_path() {
        let manifest = SyncManifest {
            root_id: "root".into(),
            revision: 1,
            entries: vec![SyncManifestEntry {
                path: "same.txt".into(),
                object: ObjectId::new("object"),
                size: 1,
                content_hash: [0; 32],
                modified_at: 1,
            }],
            tombstones: vec![SyncTombstone {
                path: "same.txt".into(),
                deleted_at: 2,
                content_hash: None,
            }],
        };
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn stale_tombstones_do_not_delete_newer_local_edits() {
        let base = SyncManifestEntry {
            path: "same.txt".into(),
            object: ObjectId::new("object"),
            size: 1,
            content_hash: [7; 32],
            modified_at: 1,
        };
        let tombstone = SyncTombstone {
            path: "same.txt".into(),
            deleted_at: 2,
            content_hash: Some([7; 32]),
        };
        assert!(remote_tombstone_covers_local(Some(&tombstone), Some(&base)));

        let stale = SyncTombstone {
            content_hash: Some([9; 32]),
            ..tombstone
        };
        assert!(!remote_tombstone_covers_local(Some(&stale), Some(&base)));

        let legacy = SyncTombstone {
            content_hash: None,
            ..stale
        };
        assert!(remote_tombstone_covers_local(Some(&legacy), Some(&base)));
    }

    #[test]
    fn tombstones_expire_after_the_retention_window() {
        let now = TOMBSTONE_RETENTION_MS + 10;
        let tombstone = |path: &str, deleted_at: u64| SyncTombstone {
            path: path.into(),
            deleted_at,
            content_hash: None,
        };
        let mut tombstones = BTreeMap::from([
            (
                "expired".to_string(),
                tombstone("expired", now - TOMBSTONE_RETENTION_MS - 1),
            ),
            (
                "edge".to_string(),
                tombstone("edge", now - TOMBSTONE_RETENTION_MS),
            ),
            ("recent".to_string(), tombstone("recent", now - 1)),
            ("future".to_string(), tombstone("future", now + 1)),
        ]);
        prune_tombstones(&mut tombstones, now);
        assert!(!tombstones.contains_key("expired"));
        assert!(tombstones.contains_key("edge"));
        assert!(tombstones.contains_key("recent"));
        assert!(tombstones.contains_key("future"));
    }

    #[test]
    fn root_names_reject_path_separators() {
        for name in ["", ".", "..", "a/b", "a\\b"] {
            assert!(validate_root_name(name).is_err());
        }
        assert!(validate_root_name("docs").is_ok());
        assert!(validate_root_name("My Documents").is_ok());
    }

    #[tokio::test]
    async fn content_chunker_round_trips_and_respects_bounds() {
        let data: Vec<u8> = (0..(3 * SYNC_CHUNK_MAX + 12_345))
            .map(|i| (i.wrapping_mul(31).wrapping_add(7) % 251) as u8)
            .collect();
        let mut chunker = SyncFileChunker::new(&data[..]);
        let mut chunks = Vec::new();
        while let Some(chunk) = chunker.next_chunk().await.unwrap() {
            chunks.push(chunk);
        }
        assert!(
            chunks.len() > 1,
            "large file should produce multiple chunks"
        );
        let mut rebuilt = Vec::new();
        for (index, chunk) in chunks.iter().enumerate() {
            assert!(chunk.len() <= SYNC_CHUNK_MAX);
            if index + 1 < chunks.len() {
                assert!(chunk.len() >= SYNC_CHUNK_MIN);
            }
            rebuilt.extend_from_slice(chunk);
        }
        assert_eq!(rebuilt, data);
    }

    #[tokio::test]
    async fn content_chunker_is_deterministic() {
        let data: Vec<u8> = (0..(2 * SYNC_CHUNK_MAX))
            .map(|i| (i.wrapping_mul(17).wrapping_add(3) % 251) as u8)
            .collect();
        let mut first = Vec::new();
        let mut chunker = SyncFileChunker::new(&data[..]);
        while let Some(chunk) = chunker.next_chunk().await.unwrap() {
            first.push(chunk);
        }
        let mut second = Vec::new();
        let mut chunker = SyncFileChunker::new(&data[..]);
        while let Some(chunk) = chunker.next_chunk().await.unwrap() {
            second.push(chunk);
        }
        assert_eq!(first, second);
    }
}
