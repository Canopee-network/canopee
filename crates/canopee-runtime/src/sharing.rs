use crate::Runtime;
use canopee_storage::{HomeIndex, ObjectId, RECORD_HOME};

impl Runtime {
    // ---- network sharing ("nothing is shared-by-default") ----

    /// Announces this node as a DHT provider of `id` and marks the object as
    /// shared, so the object-exchange protocol serves it to any peer that
    /// asks.
    pub async fn announce(&self, id: ObjectId) -> anyhow::Result<()> {
        self.shared.insert(&id).await;
        self.network.announce(id).await
    }

    /// Marks an object as servable without announcing it on the DHT — peers
    /// who learn the id out of band can fetch it, but it isn't discoverable.
    pub async fn mark_shared(&self, id: &ObjectId) {
        self.shared.insert(id).await;
    }

    /// `true` if `id` is currently served to the network (explicitly shared,
    /// or cached from a peer).
    pub async fn is_shared(&self, id: &ObjectId) -> bool {
        self.shared.contains(id).await || self.cache.is_cached(id).await
    }

    /// Loads the user's latest `HomeIndex` from the shared store, via the
    /// `(owner, "home")` record.
    pub async fn load_home_index(&self) -> anyhow::Result<Option<HomeIndex>> {
        let record = match self.resolve_pointer(self.identity.id(), RECORD_HOME).await? {
            Some(r) => r,
            None => return Ok(None),
        };
        let object = self.storage.get_verified(&record.manifest).await?;
        Ok(Some(object.decode()?))
    }

    /// Publishes a new `HomeIndex` and repoints `(owner, "home")`.
    ///
    /// Saving an index also reconciles network visibility: entries that newly
    /// became `shared: true` are marked shared and announced as providers;
    /// entries that flipped back to `shared: false` are withdrawn.
    pub async fn save_home_index(&self, index: &HomeIndex) -> anyhow::Result<ObjectId> {
        let previous = self.load_home_index().await?;
        let version = previous.as_ref().map(|i| i.version + 1).unwrap_or(1);
        let mut index = index.clone();
        index.version = version;
        let object = index.to_object(&self.identity)?;
        let id = object.id.clone();
        self.storage.put_verified(&object).await?;
        self.publish_pointer(RECORD_HOME, id.clone()).await?;
        self.reconcile_shared(previous.as_ref(), Some(&index)).await;
        Ok(id)
    }

    /// Flips one home entry's `shared` flag and republishes the index —
    /// the explicit "share this on the network" / "stop sharing" action for
    /// a single file or picture. Returns the new index object id.
    pub async fn set_home_entry_shared(
        &self,
        name: &str,
        shared: bool,
    ) -> anyhow::Result<ObjectId> {
        let mut index = self
            .load_home_index()
            .await?
            .ok_or_else(|| anyhow::anyhow!("no home index yet — save one before sharing entries"))?;
        let entry = index
            .entries
            .iter_mut()
            .find(|e| e.name == name)
            .ok_or_else(|| anyhow::anyhow!("no home entry named {name:?}"))?;
        let object = entry.object.clone();
        entry.shared = shared;
        let index_id = self.save_home_index(&index).await?;
        if shared {
            self.publish_entry_pointer(name, &object).await;
        }
        Ok(index_id)
    }

    /// Shares a stored object under `name`: upserts a `shared: true` entry in
    /// the home index (creating the index on first use) and announces the
    /// object as a DHT provider. This is the one-call "put this file on the
    /// network" action. Returns the new index object id.
    pub async fn share_object(
        &self,
        name: &str,
        id: &ObjectId,
        app: Option<String>,
    ) -> anyhow::Result<ObjectId> {
        let object = self.storage.get_verified(id).await?;
        let object_type = object.object_type();
        let mut index = self.load_home_index().await?.unwrap_or(HomeIndex {
            profile: None,
            contacts: None,
            entries: vec![],
            version: 0,
        });
        match index.entries.iter_mut().find(|e| e.name == name) {
            Some(entry) => {
                entry.object = id.clone();
                entry.object_type = object_type;
                entry.shared = true;
                entry.app = app;
            }
            None => index.entries.push(canopee_storage::HomeEntry {
                name: name.to_string(),
                object: id.clone(),
                object_type,
                shared: true,
                app,
            }),
        }
        let index_id = self.save_home_index(&index).await?;
        self.publish_entry_pointer(name, id).await;
        Ok(index_id)
    }

    /// Publishes the `(owner, "entry:<name>")` pointer record pointing at
    /// `id`, so remote peers can resolve a shared entry by its human name.
    async fn publish_entry_pointer(&self, name: &str, id: &ObjectId) {
        if let Err(e) = self.publish_pointer(&format!("entry:{name}"), id.clone()).await {
            tracing::warn!("publishing entry pointer {name:?} failed: {e}");
        }
    }

    /// Diffs the `shared` flags between two home-index versions and applies
    /// the result to the serving set: newly shared objects are announced,
    /// newly unshared ones are withdrawn.
    pub(crate) async fn reconcile_shared(
        &self,
        previous: Option<&HomeIndex>,
        current: Option<&HomeIndex>,
    ) {
        let shared_ids = |index: Option<&HomeIndex>| -> std::collections::HashSet<ObjectId> {
            index
                .map(|i| {
                    i.entries
                        .iter()
                        .filter(|e| e.shared)
                        .map(|e| e.object.clone())
                        .collect()
                })
                .unwrap_or_default()
        };
        let before = shared_ids(previous);
        let after = shared_ids(current);
        for id in after.difference(&before) {
            self.shared.insert(id).await;
            let network = self.network.clone();
            let id = id.clone();
            tokio::spawn(async move {
                if let Err(e) = network.announce(id.clone()).await {
                    tracing::warn!("announce of shared entry {id} failed: {e}");
                }
            });
        }
        for id in before.difference(&after) {
            self.shared.remove(id).await;
            let network = self.network.clone();
            let id = id.clone();
            tokio::spawn(async move {
                let _ = network.unannounce(id).await;
            });
        }
    }

    /// Fetches `object_id` from the network and imports it into the local
    /// store, verifying its signature along the way. Tries DHT providers
    /// first, then `from`. Returns the imported object.
    pub async fn fetch_object(
        &self,
        object_id: ObjectId,
        from: Option<canopee_network::PeerId>,
    ) -> anyhow::Result<canopee_storage::Object> {
        if let Ok(object) = self.storage.get_verified(&object_id).await {
            return Ok(object);
        }
        let providers = self.network.find_providers(object_id.clone()).await?;
        for provider in providers {
            match self.network.get_object(provider, object_id.clone()).await {
                Ok(bundle) => {
                    let object = bundle.object.clone();
                    self.import(bundle).await?;
                    return Ok(object);
                }
                Err(e) => tracing::warn!("fetch from {provider} failed: {e}"),
            }
        }
        if let Some(peer) = from {
            let bundle = self.network.get_object(peer, object_id).await?;
            let object = bundle.object.clone();
            self.import(bundle).await?;
            return Ok(object);
        }
        Err(anyhow::anyhow!("no provider for object {object_id}"))
    }
}
