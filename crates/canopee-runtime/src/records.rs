use crate::{normalize_username, owner_peer_id, Runtime};
use canopee_identity::IdentityId;
use canopee_network::PeerId;
use canopee_storage::{
    AppPointerRecord, ContactList, DeviceEntry, DeviceList, Export, Object, ObjectType,
    RECORD_CONTACTS, RECORD_DEVICES, RECORD_PROFILE, RECORD_USERNAME, USERNAME_REGISTRY_PREFIX,
};
use time::OffsetDateTime;

impl Runtime {
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
        // Circuit-connected-peers-first delivery: wrap the signed record bytes
        // in a content-addressed AppPointer Object and actively Store-push it
        // into every connected peer's store — the same replication path
        // `fetch_object` pulls from — so a paired device (or any connected
        // peer) can refresh this pointer deterministically instead of racing
        // an empty DHT provider sweep behind a shared relay. Best-effort: a
        // failing push is logged and the DHT put below remains the fallback.
        if let Err(e) = self.push_pointer_record(bytes.clone()).await {
            tracing::warn!("pushing pointer record ({name}) to connected peers failed: {e}");
        }
        // Fire-and-forget DHT publication: best-effort, logged on failure.
        let network = self.network.clone();
        tokio::spawn(async move {
            if let Err(e) = network.put_record(key, bytes).await {
                tracing::warn!("DHT put_record failed (record cached locally): {e}");
            }
        });
        Ok(())
    }

    /// Wraps the signed `AppPointerRecord` bytes in a verified AppPointer
    /// Object and pushes it into every connected peer's store (mirroring
    /// `concat_objects`). A local copy is kept so resolution on any device of
    /// this identity sees the same record deterministically. The object's id
    /// is the content hash of the record bytes, so a recipient verifies it
    /// against the owner's identity on arrival.
    async fn push_pointer_record(&self, bytes: Vec<u8>) -> anyhow::Result<()> {
        let object = Object::new(&self.identity, bytes, ObjectType::AppPointer);
        self.storage.put_verified(&object).await?;
        self.network.replicate_object(object.export()?, None).await?;
        Ok(())
    }

    /// Scans the content store for the newest verified AppPointer Object
    /// carrying a signed `(owner, name)` pointer — i.e. one that the owner
    /// Store-pushed into a connected peer's store at publish time. Used to
    /// resolve pointers without gambling on the DHT. Returns `None` when this
    /// device never received one for the key.
    pub(crate) async fn latest_pointer_record(
        &self,
        owner: &IdentityId,
        name: &str,
    ) -> Option<AppPointerRecord> {
        let ids = self.storage.list().await.ok()?;
        let mut newest: Option<AppPointerRecord> = None;
        for id in ids {
            let object = self.storage.get_verified(&id).await.ok()?;
            if object.payload.owner != *owner || object.object_type() != ObjectType::AppPointer {
                continue;
            }
            let record: AppPointerRecord = object.decode().ok()?;
            if record.name != name || !record.verify() {
                continue;
            }
            match &newest {
                Some(current) if current.published_at >= record.published_at => {}
                _ => newest = Some(record),
            }
        }
        newest
    }

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
        let cache_path = self
            .config
            .records_path()
            .join(format!("{}.record", hex::encode(&key)));
        let from_cache = tokio::fs::read(&cache_path).await.ok().and_then(|bytes| {
            let record: AppPointerRecord = bincode::deserialize(&bytes).ok()?;
            (record.owner == *owner && record.name == name && record.verify()).then_some(record)
        });
        if let Some(record) = from_cache {
            return Ok(Some(record));
        }
        // Circuit-connected-peers-first: the owner Store-pushed its newest
        // signed pointer-object into connected peers' stores at publish time,
        // so check that content store before falling back to the DHT. Cache
        // the find so later resolves are instant.
        if let Some(record) = self.latest_pointer_record(owner, name).await {
            if let Ok(bytes) = bincode::serialize(&record) {
                let _ = tokio::fs::create_dir_all(&self.config.records_path()).await;
                let _ = tokio::fs::write(&cache_path, bytes).await;
            }
            return Ok(Some(record));
        }
        // A DHT error or timeout (offline, no bootstrap peers yet) resolves
        // as "not found" rather than failing: a missing network must not
        // break loading the user's local data.
        let dht_bytes = match tokio::time::timeout(
            crate::RESOLVE_DHT_TIMEOUT,
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
                tracing::warn!(
                    "DHT get_record timed out after {:?}",
                    crate::RESOLVE_DHT_TIMEOUT
                );
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
    pub async fn load_profile(&self) -> anyhow::Result<Option<canopee_storage::Profile>> {
        let record = match self
            .resolve_pointer(self.identity.id(), RECORD_PROFILE)
            .await?
        {
            Some(r) => r,
            None => return Ok(None),
        };
        let object = self.storage.get_verified(&record.manifest).await?;
        Ok(Some(object.decode()?))
    }

    /// Publishes a new `Profile` object and repoints `(owner, "profile")`.
    pub async fn save_profile(
        &self,
        profile: &canopee_storage::Profile,
    ) -> anyhow::Result<ObjectId> {
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
        let record = canopee_storage::UsernameRecord {
            username: username.clone(),
            version,
        };
        let object = record.to_object(&self.identity)?;
        let id = object.id.clone();
        self.storage.put_verified(&object).await?;
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
                if let Err(e) = network.put_record(registry_key, value).await {
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
    ) -> anyhow::Result<Option<canopee_storage::UsernameRecord>> {
        let object = self.resolve_owner_object(owner, RECORD_USERNAME).await?;
        let Some(object) = object else {
            return Ok(None);
        };
        Ok(Some(object.decode()?))
    }

    /// Resolves `owner`'s signed `Profile` (display name, DH key, avatar)
    /// via the `(owner, "profile")` record, fetching from the network when
    /// it isn't cached locally.
    pub async fn resolve_profile(
        &self,
        owner: &IdentityId,
    ) -> anyhow::Result<Option<canopee_storage::Profile>> {
        let object = self.resolve_owner_object(owner, RECORD_PROFILE).await?;
        let Some(object) = object else {
            return Ok(None);
        };
        Ok(Some(object.decode()?))
    }

    /// Resolves the signed, verified object a peer publishes under
    /// `(owner, name)` (e.g. `profile` or `username`), fetching it from the
    /// network when it isn't cached locally. Returns `None` when there is no
    /// record or the object doesn't verify against `owner`.
    pub(crate) async fn resolve_owner_object(
        &self,
        owner: &IdentityId,
        name: &str,
    ) -> anyhow::Result<Option<canopee_storage::Object>> {
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
    /// resolve the identity it carries (via the `device:<peer-id>` registry,
    /// falling back to the pre-device-key assumption that the device id *is*
    /// the identity's embedded peer id) and then its signed username/profile
    /// display name, pushing the result back into the network manager's peer
    /// map so `peers()` and the UI can show friendly names instead of raw
    /// peer ids.
    pub async fn enrich_peers(&self) -> anyhow::Result<()> {
        const ENRICH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

        let peers = self.network.peers().await?;
        let resolutions = peers.into_iter().map(|peer| {
            let peer_id = peer.peer_id;
            async move {
                let (identity, resolved) = match tokio::time::timeout(
                    ENRICH_TIMEOUT,
                    self.resolve_device_identity(&peer_id),
                )
                .await
                {
                    Ok(Ok(Some(id))) => (Some(id), true),
                    _ => (
                        Some(IdentityId::new(format!("canopee://identity/{peer_id}"))),
                        false,
                    ),
                };
                let (username, display_name) = tokio::join!(
                    async {
                        match &identity {
                            Some(id) => {
                                tokio::time::timeout(ENRICH_TIMEOUT, self.resolve_username(id))
                                    .await
                                    .ok()
                                    .and_then(|r| r.ok())
                                    .flatten()
                                    .map(|u| u.username)
                            }
                            None => None,
                        }
                    },
                    async {
                        match &identity {
                            Some(id) => {
                                tokio::time::timeout(ENRICH_TIMEOUT, self.resolve_profile(id))
                                    .await
                                    .ok()
                                    .and_then(|r| r.ok())
                                    .flatten()
                                    .map(|p| p.display_name)
                            }
                            None => None,
                        }
                    }
                );
                (peer_id, identity, username, display_name, resolved)
            }
        });
        let results = futures::future::join_all(resolutions).await;
        for (peer_id, identity, username, display_name, resolved) in results {
            if username.is_none() && display_name.is_none() && !resolved {
                continue;
            }
            let _ = self
                .network
                .set_peer_meta(peer_id, identity, username, display_name)
                .await;
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
        // Circuit-connected-peers-first: each publisher Store-pushes its
        // signed `(owner, "username")` pointer-object into connected peers'
        // stores, so reverse-resolve by scanning those delivered claims
        // (verifying each candidate publishes exactly this name) before
        // gambling on the DHT registry.
        if let Some(owner) = self.owner_claiming_username(&username).await {
            return Ok(Some(owner));
        }
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
        match self.resolve_username(&owner).await? {
            Some(record) if record.username == username => Ok(Some(owner)),
            Some(_) => {
                tracing::warn!(
                    "username {username} claimed by an owner publishing a different name"
                );
                Ok(None)
            }
            None => Ok(None),
        }
    }

    /// Scans the content store for delivered AppPointer Objects carrying an
    /// `(owner, "username")` pointer, and returns the first owner whose
    /// verified `UsernameRecord` matches `username`. Successfully mirrors the
    /// DHT `username:<name>` registry whenever the publisher's Store-push
    /// landed in a connected peer.
    async fn owner_claiming_username(&self, username: &str) -> Option<IdentityId> {
        let ids = self.storage.list().await.ok()?;
        let mut owners: Vec<IdentityId> = Vec::new();
        for id in ids {
            let object = self.storage.get_verified(&id).await.ok()?;
            if object.object_type() != ObjectType::AppPointer {
                continue;
            }
            let record: AppPointerRecord = object.decode().ok()?;
            if record.name != RECORD_USERNAME || !record.verify() {
                continue;
            }
            if !owners.contains(&record.owner) {
                owners.push(record.owner);
            }
        }
        for owner in owners {
            match self.resolve_username(&owner).await.ok().flatten() {
                Some(record) if record.username == username => return Some(owner),
                _ => {}
            }
        }
        None
    }

    /// The DHT record key mapping a device's network `PeerId` back to the
    /// identity it carries: `device:<peer-id>` → `canopee://identity/<id>`.
    pub(crate) fn device_registry_key(device_id: &PeerId) -> Vec<u8> {
        use canopee_storage::DEVICE_REGISTRY_PREFIX;
        format!("{DEVICE_REGISTRY_PREFIX}{device_id}").into_bytes()
    }

    /// Loads the list of devices carrying an identity, via the
    /// `(owner, "devices")` record (fetching from the network when not
    /// cached). `None` until the owner has registered at least one device.
    pub async fn load_device_list(&self, owner: &IdentityId) -> anyhow::Result<Option<DeviceList>> {
        let object = self.resolve_owner_object(owner, RECORD_DEVICES).await?;
        let Some(object) = object else {
            return Ok(None);
        };
        Ok(Some(object.decode()?))
    }

    /// Publishes a new `DeviceList` snapshot and repoints
    /// `(owner, "devices")`.
    pub async fn save_device_list(&self, list: &DeviceList) -> anyhow::Result<ObjectId> {
        let version = self
            .load_device_list(self.identity.id())
            .await?
            .map(|l| l.version + 1)
            .unwrap_or(1);
        let mut list = list.clone();
        list.version = version;
        let object = list.to_object(&self.identity)?;
        let id = object.id.clone();
        self.storage.put_verified(&object).await?;
        self.announce(id.clone()).await?;
        self.publish_pointer(RECORD_DEVICES, id.clone()).await?;
        Ok(id)
    }

    /// Adds (or refreshes) one device entry on the owner's device list and
    /// republishes it. Re-registering an existing device keeps its original
    /// `added_at`.
    pub async fn add_device(
        &self,
        device_id: &str,
        device_name: &str,
    ) -> anyhow::Result<DeviceList> {
        let mut list = self
            .load_device_list(self.identity.id())
            .await?
            .unwrap_or(DeviceList {
                devices: vec![],
                version: 0,
            });
        let added_at = list.by_device_id(device_id).and_then(|d| d.added_at);
        if !list.devices.iter().any(|d| d.device_id == device_id) {
            list.devices.push(DeviceEntry {
                device_id: device_id.to_string(),
                device_name: device_name.to_string(),
                added_at: added_at.or(Some(OffsetDateTime::now_utc())),
            });
        } else if let Some(entry) = list.devices.iter_mut().find(|d| d.device_id == device_id) {
            entry.device_name = device_name.to_string();
            entry.added_at = added_at.or(Some(OffsetDateTime::now_utc()));
        }
        let _ = self.save_device_list(&list).await?;
        Ok(list)
    }

    /// Removes one device from the owner's device list and republishes it.
    pub async fn remove_device(&self, device_id: &str) -> anyhow::Result<DeviceList> {
        let mut list = self
            .load_device_list(self.identity.id())
            .await?
            .unwrap_or(DeviceList {
                devices: vec![],
                version: 0,
            });
        list.devices.retain(|d| d.device_id != device_id);
        let _ = self.save_device_list(&list).await?;
        Ok(list)
    }

    /// Registers this device with its identity: upserts the device into the
    /// `(owner, "devices")` list and publishes the `device:<peer-id>` →
    /// identity registry record so other peers can reverse-resolve the
    /// machine. Best-effort by design.
    pub(crate) async fn register_device(&self) {
        let peer_id = self.device_key.peer_id();
        if let Err(e) = self
            .add_device(&peer_id.to_string(), self.device_key.device_name())
            .await
        {
            tracing::warn!("registering this device in the device list failed: {e}");
        }
        self.publish_device_registry().await;
    }

    /// Schedules one DHT re-publication of this device's registry record,
    /// device-list pointer, and device-list provider announcement after the
    /// network becomes reachable.
    pub(crate) fn schedule_device_publish(&self) {
        use canopee_storage::AppPointerRecord as Apr;

        let network = self.network.clone();
        let identity_id = self.identity.id().clone();
        let records_path = self.config.records_path();
        let registry_key = Self::device_registry_key(&self.device_key.peer_id());
        let registry_value = identity_id.to_string().into_bytes();

        tokio::spawn(async move {
            for _ in 0..50 {
                if let Ok(peers) = network.peers().await {
                    if !peers.is_empty() {
                        break;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }

            if let Err(e) = network.put_record(registry_key, registry_value).await {
                tracing::warn!("device registry re-publication failed: {e}");
            }

            let key = Apr::key(&identity_id, RECORD_DEVICES);
            let path = records_path.join(format!("{}.record", hex::encode(&key)));
            if let Ok(bytes) = tokio::fs::read(&path).await {
                if let Ok(record) = bincode::deserialize::<Apr>(&bytes) {
                    if record.verify() && record.owner == identity_id {
                        let _ = network.put_record(key, bytes).await;
                        let _ = network.announce(record.manifest.clone()).await;
                    }
                }
            }
        });
    }

    /// Publishes `device:<own-peer-id>` → `canopee://identity/<own-id>` both
    /// durably on this machine (the local record cache) and best-effort on
    /// the DHT.
    async fn publish_device_registry(&self) {
        let key = Self::device_registry_key(&self.device_key.peer_id());
        let value = self.identity.id().to_string().into_bytes();
        self.publish_record(key, value).await;
    }

    /// Reverse-resolves a device's network `PeerId` back to the identity it
    /// carries, from the `device:<peer-id>` registry (local cache first,
    /// then DHT). `None` for devices that never registered.
    pub async fn resolve_device_identity(
        &self,
        device_id: &PeerId,
    ) -> anyhow::Result<Option<IdentityId>> {
        let key = Self::device_registry_key(device_id);
        let Some(bytes) = self.resolve_record(&key).await? else {
            return Ok(None);
        };
        let id_string = String::from_utf8(bytes)
            .map_err(|_| anyhow::anyhow!("device registry record is not valid utf-8"))?;
        Ok(Some(IdentityId::new(id_string)))
    }

    /// Resolves which device of `owner` to dial, via the `(owner, "devices")`
    /// list.
    pub async fn resolve_device_peer_id(
        &self,
        owner: &IdentityId,
    ) -> anyhow::Result<Option<PeerId>> {
        if owner == self.identity.id() {
            return Ok(Some(self.device_key.peer_id()));
        }
        let Some(list) = self.load_device_list(owner).await? else {
            return Ok(None);
        };
        Ok(list.devices.iter().find_map(|d| d.device_id.parse().ok()))
    }

    /// Writes `key → value` durably to the local record cache (awaited) and
    /// best-effort to the DHT (fire-and-forget).
    pub(crate) async fn publish_record(&self, key: Vec<u8>, value: Vec<u8>) {
        if let Err(e) = self.cache_record(&key, &value).await {
            tracing::warn!("caching record {key:?} failed: {e}");
        }
        let network = self.network.clone();
        tokio::spawn(async move {
            if let Err(e) = network.put_record(key, value).await {
                tracing::warn!("DHT put_record failed (record cached locally): {e}");
            }
        });
    }

    /// Resolves `key` from the local record cache first, then the DHT
    /// (bounded — see `RESOLVE_DHT_TIMEOUT`).
    pub(crate) async fn resolve_record(&self, key: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
        let cache_path = self
            .config
            .records_path()
            .join(format!("{}.record", hex::encode(key)));
        if let Ok(bytes) = tokio::fs::read(&cache_path).await {
            return Ok(Some(bytes));
        }
        let dht = match tokio::time::timeout(
            crate::RESOLVE_DHT_TIMEOUT,
            self.network.get_record(key.to_vec()),
        )
        .await
        {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(e)) => {
                tracing::warn!("DHT get_record failed: {e}");
                None
            }
            Err(_) => {
                tracing::warn!(
                    "DHT get_record timed out after {:?}",
                    crate::RESOLVE_DHT_TIMEOUT
                );
                None
            }
        };
        if let Some(value) = &dht {
            let _ = self.cache_record(key, value).await;
        }
        Ok(dht)
    }

    pub(crate) async fn cache_record(&self, key: &[u8], value: &[u8]) -> anyhow::Result<()> {
        let cache_path = self.config.records_path();
        tokio::fs::create_dir_all(&cache_path).await?;
        let path = cache_path.join(format!("{}.record", hex::encode(key)));
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, value).await?;
        tokio::fs::rename(&tmp, path).await?;
        Ok(())
    }

    /// Loads the user's latest `ContactList` from the shared store, via the
    /// `(owner, "contacts")` record.
    pub async fn load_contact_list(&self) -> anyhow::Result<Option<ContactList>> {
        let record = match self
            .resolve_pointer(self.identity.id(), RECORD_CONTACTS)
            .await?
        {
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
}

use canopee_storage::ObjectId;
