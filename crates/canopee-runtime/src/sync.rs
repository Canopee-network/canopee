use crate::Runtime;
use canopee_network::PeerId;
use canopee_protocol::SyncResult;
use canopee_storage::{AppPointerRecord, RECORD_CONTACTS, RECORD_DEVICES, RECORD_PROFILE};

impl Runtime {
    /// How often the background sync task refreshes user records from the
    /// DHT.
    pub(crate) const PERIODIC_SYNC_INTERVAL: std::time::Duration =
        std::time::Duration::from_secs(30);

    /// Background task that periodically refreshes the identity-scoped user
    /// records (profile, contacts, devices) from the DHT. Only runs when the
    /// node is online (`started`) and has at least one connected peer.
    pub(crate) fn spawn_periodic_sync(&self) {
        let runtime = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Self::PERIODIC_SYNC_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let started = runtime.state.read().await.started;
                if !started {
                    continue;
                }
                let peer_count = runtime.network.peers().await.map(|p| p.len()).unwrap_or(0);
                if peer_count == 0 {
                    continue;
                }
                match runtime.sync_with_all_devices().await {
                    Ok(result) if result.any_updated() => {
                        tracing::info!("periodic sync: {result:?}");
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("periodic sync failed: {e}"),
                }
            }
        });
    }

    /// Refreshes this node's user records (profile, contacts, devices) from
    /// the network.
    pub async fn sync_with_peer(&self, peer_id: PeerId) -> anyhow::Result<SyncResult> {
        tracing::info!("syncing records (peer hint: {peer_id})");
        self.sync_records().await
    }

    /// Refreshes this node's user records from every device in its
    /// `(owner, "devices")` list.
    pub async fn sync_with_all_devices(&self) -> anyhow::Result<SyncResult> {
        let devices = self.load_device_list(self.identity.id()).await?;
        tracing::info!(
            "syncing records across {} registered device(s)",
            devices.map(|d| d.devices.len()).unwrap_or(0)
        );
        self.sync_records().await
    }

    /// Refreshes the three user records (profile, contacts, devices) from
    /// the DHT, last-writer-wins by the signed pointer's `published_at`.
    async fn sync_records(&self) -> anyhow::Result<SyncResult> {
        let mut result = SyncResult::default();
        for name in [RECORD_PROFILE, RECORD_CONTACTS, RECORD_DEVICES] {
            if self.sync_record(name).await? {
                match name {
                    RECORD_PROFILE => result.profile_updated = true,
                    RECORD_CONTACTS => result.contacts_updated = true,
                    RECORD_DEVICES => result.devices_updated = true,
                    _ => {}
                }
            }
        }
        Ok(result)
    }

    /// Refreshes one user record: first from the circuit-connected-peers arm
    /// (the publisher Store-pushed its signed pointer-object into connected
    /// peers' stores at publish time), falling back to the DHT only when no
    /// connected peer delivered one, last-writer-wins by the signed pointer's
    /// `published_at`. Returns `true` when the record was refreshed.
    async fn sync_record(&self, name: &str) -> anyhow::Result<bool> {
        let owner = self.identity.id().clone();
        let key = AppPointerRecord::key(&owner, name);
        let cache_path = self
            .config
            .records_path()
            .join(format!("{}.record", hex::encode(&key)));
        let local_record: Option<AppPointerRecord> = tokio::fs::read(&cache_path)
            .await
            .ok()
            .and_then(|bytes| bincode::deserialize(&bytes).ok())
            .filter(|r: &AppPointerRecord| r.owner == owner && r.name == name && r.verify());
        // Circuit-connected peers first: the publisher's Store-push landed
        // the newest signed record for this (owner, name) in our content
        // store, so refresh from it without a DHT provider sweep.
        if let Some(pushed) = self.latest_pointer_record(&owner, name).await {
            let is_newer = match &local_record {
                Some(local) => pushed.published_at > local.published_at,
                None => true,
            };
            if is_newer {
                self.fetch_object(pushed.manifest.clone(), None).await?;
                self.cache_record(&key, &bincode::serialize(&pushed)?).await?;
                tracing::info!(
                    "sync: refreshed {name} from pushed pointer (published_at {})",
                    pushed.published_at
                );
                return Ok(true);
            }
        }
        // DHT fallback: reachable only when no connected peer delivered a
        // newer pointer (e.g. this device was offline when the peer
        // published it). Unreachable DHT resolves as "not found".
        let dht_bytes = match tokio::time::timeout(
            crate::RESOLVE_DHT_TIMEOUT,
            self.network.get_record(key.clone()),
        )
        .await
        {
            Ok(Ok(Some(bytes))) => bytes,
            Ok(Ok(None)) => return Ok(false),
            Ok(Err(e)) => {
                tracing::warn!("sync: DHT get_record for {name} failed: {e}");
                return Ok(false);
            }
            Err(_) => {
                tracing::warn!("sync: DHT get_record for {name} timed out");
                return Ok(false);
            }
        };
        let dht_record: AppPointerRecord =
            match bincode::deserialize::<AppPointerRecord>(&dht_bytes) {
                Ok(r) if r.owner == owner && r.name == name && r.verify() => r,
                _ => return Ok(false),
            };
        let is_newer = match &local_record {
            Some(local) => dht_record.published_at > local.published_at,
            None => true,
        };
        if !is_newer {
            return Ok(false);
        }
        self.fetch_object(dht_record.manifest.clone(), None).await?;
        self.cache_record(&key, &bincode::serialize(&dht_record)?).await?;
        tracing::info!(
            "sync: refreshed {name} (published_at {})",
            dht_record.published_at
        );
        Ok(true)
    }
}
