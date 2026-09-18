use canopee_protocol::{DeviceInfo, NodeCommand, NodeResponse};
use canopee_runtime::Runtime;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;
use tokio::task::JoinSet;

/// Maximum bytes the cache may hold before the periodic sweep starts evicting
/// least-recently-served cached objects. Default 256 MiB; override with
/// `CANOPEE_CACHE_MAX_MB`.
fn cache_cap_bytes() -> u64 {
    let mb = std::env::var("CANOPEE_CACHE_MAX_MB")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok());
    let mb = mb.unwrap_or(256);
    mb.saturating_mul(1024 * 1024)
}

#[derive(Clone)]
pub struct Node {
    runtime: Arc<Runtime>,
    shutdown: broadcast::Sender<()>,
}

impl Node {
    pub fn new(runtime: Runtime) -> Self {
        let (shutdown, _) = broadcast::channel(1);
        Self {
            runtime: Arc::new(runtime),
            shutdown,
        }
    }

    pub async fn open() -> anyhow::Result<Self> {
        let runtime = Runtime::open().await?;
        Ok(Self::new(runtime))
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        let socket_path = self.runtime.node_socket_path();
        if socket_path.exists() {
            tokio::fs::remove_file(&socket_path).await?;
        }
        let listener = UnixListener::bind(&socket_path)?;
        println!("Canopee node listening on {:?}", socket_path);
        let mut shutdown = self.shutdown.subscribe();
        let mut tasks: JoinSet<()> = JoinSet::new();
        self.runtime.mark_started().await?;
        let cap = cache_cap_bytes();
        let mut sweep = tokio::time::interval(std::time::Duration::from_secs(30));

        loop {
            tokio::select! {
                _ = sweep.tick() => {
                    self.evict_if_over_cap(cap).await;
                }
                result = listener.accept() => {
                    let (stream, _) = result?;
                    let node = self.clone();
                    tasks.spawn(async move {
                        if let Err(e) = node.handle_connection(stream).await {
                            eprintln!("Connection error: {}", e);
                        }
                    });
                }
                result = shutdown.recv() => {
                    println!("Shutdown received: {:?}", result);
                    tasks.abort_all();
                    while let Some(result) = tasks.join_next().await {
                        match result {
                            Ok(_) => {}
                            Err(e) => {
                                if !e.is_cancelled() {
                                    eprintln!("Task failed: {}", e);
                                }
                            }
                        }
                    }
                    break;
                }
            }
        }
        tasks.abort_all();
        if socket_path.exists() {
            tokio::fs::remove_file(&socket_path).await?;
        }

        Ok(())
    }

    /// Periodic cache-maintenance sweep: if the total bytes held by cached
    /// (non-owned) objects exceeds `cap`, evict least-recently-served cached
    /// objects until it's back under the cap. Never touches objects owned by
    /// this node's own identity.
    async fn evict_if_over_cap(&self, cap: u64) {
        loop {
            let total = self.runtime.cached_bytes().await;
            if total <= cap {
                break;
            }
            let Some(object) = self.runtime.lru_cached().await.into_iter().next() else {
                break;
            };
            let id = object.id.clone();
            println!(
                "Evicting cached object {id} ({} bytes; cached total {total} > cap {cap})",
                object.payload.metadata.size
            );
            if let Err(e) = self.runtime.evict(&id).await {
                eprintln!("Failed to evict cached object {id}: {e}");
                break;
            }
        }
    }

    async fn read_command(&self, stream: &mut UnixStream) -> anyhow::Result<NodeCommand> {
        println!("stream: {:? }", stream);
        let size = stream.read_u32().await?;
        let mut buffer = vec![0u8; size as usize];
        stream.read_exact(&mut buffer).await?;
        let command = bincode::deserialize(&buffer)?;

        Ok(command)
    }

    async fn write_response(
        &self,
        stream: &mut UnixStream,
        response: NodeResponse,
    ) -> anyhow::Result<()> {
        println!("response: {:? }", response);
        let bytes = bincode::serialize(&response)?;
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await?;

        Ok(())
    }

    async fn handle_connection(&self, mut stream: UnixStream) -> anyhow::Result<()> {
        let command: NodeCommand = self.read_command(&mut stream).await?;

        if let NodeCommand::Subscribe { topic } = command {
            return self.handle_subscribe(stream, topic).await;
        }

        let should_shutdown = matches!(command, NodeCommand::Shutdown);
        let response = self.handle(command).await;
        if let Err(e) = self.write_response(&mut stream, response).await {
            eprintln!("Failed writing response: {}", e);
        }
        if should_shutdown {
            println!("Sending shutdown signal");
            let _ = self.shutdown.send(());
        }
        Ok(())
    }

    async fn handle_subscribe(&self, mut stream: UnixStream, topic: String) -> anyhow::Result<()> {
        let mut receiver = match self.runtime.network.subscribe(&topic).await {
            Ok(receiver) => receiver,
            Err(e) => {
                let _ = self
                    .write_response(
                        &mut stream,
                        NodeResponse::Error {
                            message: e.to_string(),
                        },
                    )
                    .await;
                return Ok(());
            }
        };
        self.write_response(&mut stream, NodeResponse::Subscribed)
            .await?;

        let mut shutdown = self.shutdown.subscribe();
        loop {
            tokio::select! {
                message = receiver.recv() => {
                    let Ok(message) = message else { break };
                    if message.topic != topic {
                        continue;
                    }
                    let response = NodeResponse::PubSub(canopee_protocol::PubSubMessage {
                        topic: message.topic,
                        source: message.source.map(|p| p.to_string()),
                        data: message.data,
                    });
                    if self.write_response(&mut stream, response).await.is_err() {
                        break;
                    }
                }
                _ = shutdown.recv() => break,
            }
        }

        let _ = self.runtime.network.unsubscribe(&topic).await;
        Ok(())
    }

    pub async fn handle(&self, command: NodeCommand) -> NodeResponse {
        match command {
            NodeCommand::Identity => {
                let identity_id = self.runtime.identity.identity_id.clone();
                NodeResponse::Identity { identity_id }
            }

            NodeCommand::Put { data, name } => match self.runtime.put(data, name).await {
                Ok(id) => NodeResponse::ObjectCreated { id },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::SetName { id, name } => {
                match self.runtime.storage.set_name(&id, &name).await {
                    Ok(()) => NodeResponse::NameSet,
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::Get { id } => match self.runtime.get(&id).await {
                Ok(object) => NodeResponse::Object { object },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::List => match self.runtime.list().await {
                Ok(objects) => NodeResponse::Objects { objects },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::Status => {
                let objects = self.runtime.list().await.unwrap_or_default().len();
                let peers = self.runtime.network.peers().await.unwrap_or_default().len();
                NodeResponse::Status {
                    identity: self.runtime.identity().id().to_string(),
                    objects,
                    peers,
                }
            }

            NodeCommand::Export { id } => match self.runtime.export_to_file(&id).await {
                Ok(bundle) => NodeResponse::Exported { bundle },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::Import { bundle } => match self.runtime.import(bundle).await {
                Ok(_) => NodeResponse::Imported,
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::Shutdown => NodeResponse::ShutdownAccepted,

            NodeCommand::Dial { addr } => match addr.parse() {
                Ok(addr) => match self.runtime.network.dial(addr).await {
                    Ok(_) => NodeResponse::Dialed,
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                },
                Err(e) => NodeResponse::Error {
                    message: format!("Invalid address: {e}"),
                },
            },

            NodeCommand::ListenViaRelay { relay_addr } => match relay_addr.parse() {
                Ok(relay_addr) => match self.runtime.network.listen_via_relay(relay_addr).await {
                    Ok(_) => NodeResponse::ListeningViaRelay,
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                },
                Err(e) => NodeResponse::Error {
                    message: format!("Invalid address: {e}"),
                },
            },

            NodeCommand::Publish { topic, data } => {
                match self.runtime.network.publish(&topic, data).await {
                    Ok(_) => NodeResponse::Published,
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            // Intercepted in `handle_connection` before reaching here.
            NodeCommand::Subscribe { .. } => NodeResponse::Error {
                message: "Subscribe must be handled as a streaming connection".to_string(),
            },

            NodeCommand::Peers => {
                // Best-effort enrichment: resolve connected peers' usernames
                // and display names before answering, so the CLI/UI can show
                // friendly names. Bounded internally (see `enrich_peers`).
                let _ = self.runtime.enrich_peers().await;
                match self.runtime.network.peers().await {
                    Ok(peers) => NodeResponse::Peers {
                        peers: peers
                            .into_iter()
                            .map(|peer| canopee_protocol::PeerInfo {
                                peer_id: peer.peer_id.to_string(),
                                identity: peer.identity,
                                username: peer.username,
                                display_name: peer.display_name,
                                addresses: peer
                                    .addresses
                                    .iter()
                                    .map(|a| a.to_string())
                                    .collect(),
                            })
                            .collect(),
                    },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::RelayReservations => match self.runtime.network.relay_reservations().await
            {
                Ok(reservations) => NodeResponse::RelayReservations {
                    reservations: reservations
                        .into_iter()
                        .map(|r| canopee_protocol::RelayReservationInfo {
                            relay_peer_id: r.relay_peer_id.to_string(),
                            renewal: r.renewal,
                            listen_addrs: r.listen_addrs.iter().map(|a| a.to_string()).collect(),
                        })
                        .collect(),
                },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::FindProviders { id } => {
                match self.runtime.network.find_providers(id).await {
                    Ok(peer_ids) => NodeResponse::Providers {
                        peer_ids: peer_ids.iter().map(|p| p.to_string()).collect(),
                    },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::FetchObject { peer_id, id } => match peer_id.parse() {
                Ok(peer_id) => match self.runtime.network.get_object(peer_id, id).await {
                    Ok(bundle) => NodeResponse::Exported { bundle },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                },
                Err(e) => NodeResponse::Error {
                    message: format!("Invalid peer id: {e}"),
                },
            },

            NodeCommand::Announce { id } => match self.runtime.announce(id).await {
                Ok(_) => NodeResponse::Announced,
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },
            NodeCommand::PutObject {
                data,
                object_type,
                name,
            } => {
                match self.runtime
                    .put_object(data, object_type, name)
                    .await
                {
                    Ok(object) => NodeResponse::ObjectCreated { id: object.id },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::PublishAppPointer { name, manifest } => {
                let result = async {
                    let record = canopee_storage::AppPointerRecord::sign(
                        self.runtime.identity(),
                        &name,
                        manifest,
                    )?;
                    let key = canopee_storage::AppPointerRecord::key(&record.owner, &name);
                    let value = bincode::serialize(&record)?;
                    self.runtime.network.put_record(key, value).await
                }
                .await;

                match result {
                    Ok(()) => NodeResponse::AppPointerPublished,
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::ResolveAppPointer { owner, name } => {
                let key = canopee_storage::AppPointerRecord::key(&owner, &name);
                match self.runtime.network.get_record(key).await {
                    Ok(Some(bytes)) => match bincode::deserialize(&bytes) {
                        Ok(record) => NodeResponse::AppPointer {
                            record: Some(record),
                        },
                        Err(e) => NodeResponse::Error {
                            message: format!("Corrupt app pointer record: {e}"),
                        },
                    },
                    Ok(None) => NodeResponse::AppPointer { record: None },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            // ---- user records (cache-aware Runtime layer) ----

            NodeCommand::PublishPointer { name, target } => {
                match self.runtime.publish_pointer(&name, target).await {
                    Ok(()) => NodeResponse::PointerPublished,
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::ResolvePointer { owner, name } => {
                match self.runtime.resolve_pointer(&owner, &name).await {
                    Ok(record) => NodeResponse::Pointer { record },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::SaveProfile { profile } => {
                match self.runtime.save_profile(&profile).await {
                    Ok(id) => NodeResponse::ProfileSaved { id },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::LoadProfile => match self.runtime.load_profile().await {
                Ok(profile) => NodeResponse::Profile { profile },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::SaveContactList { list } => {
                match self.runtime.save_contact_list(&list).await {
                    Ok(id) => NodeResponse::ContactListSaved { id },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::LoadContactList => match self.runtime.load_contact_list().await {
                Ok(list) => NodeResponse::ContactList { list },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::SaveHomeIndex { index } => {
                match self.runtime.save_home_index(&index).await {
                    Ok(id) => NodeResponse::HomeIndexSaved { id },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::LoadHomeIndex => match self.runtime.load_home_index().await {
                Ok(index) => NodeResponse::HomeIndex { index },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::SetHomeEntryShared { name, shared } => {
                match self.runtime.set_home_entry_shared(&name, shared).await {
                    Ok(id) => NodeResponse::HomeIndexSaved { id },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::ShareObject { name, object, app } => {
                match self.runtime.share_object(&name, &object, app).await {
                    Ok(id) => NodeResponse::HomeIndexSaved { id },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::ClaimUsername { username } => {
                match self.runtime.claim_username(&username).await {
                    Ok(()) => NodeResponse::UsernameClaimed,
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::ResolveUsername { username } => {
                match self.runtime.resolve_owner_from_username(&username).await {
                    Ok(owner) => NodeResponse::UsernameOwner { owner },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::ShowUsername => {
                match self
                    .runtime
                    .resolve_username(self.runtime.identity().id())
                    .await
                {
                    Ok(record) => NodeResponse::Username {
                        username: record.map(|r| r.username),
                    },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::ExportIdentity { passphrase } => {
                match self.runtime.export_identity(&passphrase) {
                    Ok(bytes) => NodeResponse::IdentityExported { bytes },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::ImportIdentity {
                bytes,
                passphrase,
                overwrite,
            } => match self
                .runtime
                .import_identity(&bytes, &passphrase, overwrite)
                .await
            {
                Ok(identity_id) => NodeResponse::IdentityImported { identity_id },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::Device => {
                let peer_id = self.runtime.device_key.peer_id().to_string();
                let device_name = self.runtime.device_key.device_name().to_string();
                NodeResponse::Device {
                    peer_id,
                    device_name,
                }
            }

            NodeCommand::DeviceList => {
                match self
                    .runtime
                    .load_device_list(self.runtime.identity().id())
                    .await
                {
                    Ok(list) => NodeResponse::DeviceList {
                        devices: list
                            .map(|l| {
                                l.devices
                                .into_iter()
                                .map(|d| DeviceInfo {
                                    device_id: d.device_id,
                                    device_name: d.device_name,
                                })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::ResolveOwnerDevice { owner } => {
                match self.runtime.resolve_device_peer_id(&owner).await {
                    Ok(peer_id) => NodeResponse::OwnerDevice {
                        peer_id: peer_id.map(|p| p.to_string()),
                    },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::AddDevice {
                device_id,
                device_name,
            } => match self
                .runtime
                .add_device(&device_id, &device_name)
                .await
            {
                Ok(_) => NodeResponse::DeviceAdded,
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::RemoveDevice { device_id } => {
                match self.runtime.remove_device(&device_id).await {
                    Ok(_) => NodeResponse::DeviceRemoved,
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::InitiatePairing => {
                match self.runtime.initiate_pairing().await {
                    Ok(qr) => NodeResponse::PairingQr { qr },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::CompletePairing { qr, code } => {
                match self.runtime.complete_pairing(qr, &code).await {
                    Ok(message) => NodeResponse::PairingComplete { message },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::SyncFromPeer { peer_id } => {
                match peer_id.parse() {
                    Ok(peer_id) => match self.runtime.sync_with_peer(peer_id).await {
                        Ok(result) => NodeResponse::SyncComplete { result },
                        Err(e) => NodeResponse::Error {
                            message: e.to_string(),
                        },
                    },
                    Err(e) => NodeResponse::Error {
                        message: format!("invalid peer id: {e}"),
                    },
                }
            }

            NodeCommand::SyncDeviceList => {
                match self.runtime.sync_with_all_devices().await {
                    Ok(result) => NodeResponse::SyncComplete { result },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::GrantCapability {
                subject,
                resource,
                permissions,
                expires_at,
            } => match self
                .runtime
                .grant_capability(subject, resource, permissions, expires_at)
                .await
            {
                Ok(capability) => NodeResponse::CapabilityGranted { capability },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::ListCapabilities => match self.runtime.list_capabilities().await {
                Ok(index) => NodeResponse::Capabilities { index },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::RevokeCapability { id } => {
                match self.runtime.revoke_capability(&id).await {
                    Ok(()) => NodeResponse::CapabilityRevoked,
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::CheckCapability { capability } => {
                match self.runtime.check_capability(&capability).await {
                    Ok((valid, reason)) => NodeResponse::CapabilityCheck { valid, reason },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::CheckAccess {
                subject,
                permission,
                resource,
            } => match self
                .runtime
                .check_access(&subject, permission, &resource)
                .await
            {
                Ok(allowed) => NodeResponse::AccessAllowed { allowed },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            }
        }
    }
}

#[tokio::test]
async fn test_node_put() {
    let runtime = Runtime::open().await.unwrap();
    let node = Node::new(runtime);
    let response = node
        .handle(NodeCommand::Put {
            data: b"hello".to_vec(),
            name: None,
        })
        .await;

    println!("{:?}", response);
}

#[cfg(test)]
mod eviction_tests {
    use super::*;
    use canopee_identity::Identity;
    use canopee_storage::{Export, Object, ObjectId, ObjectType};
    use std::sync::{Arc, Once};

    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static SETUP: Once = Once::new();

    fn scratch_home() -> &'static std::path::PathBuf {
        SETUP.call_once(|| {
            let home = std::env::temp_dir().join(format!("canopee_node_test_{}", std::process::id()));
            std::fs::create_dir_all(&home).unwrap();
            unsafe {
                std::env::set_var("HOME", &home);
            }
        });
        static HOME: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        HOME.get_or_init(|| {
            std::env::temp_dir().join(format!("canopee_node_test_{}", std::process::id()))
        })
    }

    async fn imported_object(runtime: &Runtime, data: &[u8]) -> (ObjectId, Object) {
        let other_dir = std::env::temp_dir()
            .join(format!("canopee_node_test_other_{}", std::process::id()));
        std::fs::create_dir_all(&other_dir).unwrap();
        let other = Arc::new(
            Identity::create(other_dir.join(format!("{}.key", data.len())).to_str().unwrap())
                .await
                .unwrap(),
        );
        let object = Object::new(&other, data.to_vec(), ObjectType::Blob);
        let id = object.id.clone();
        runtime.import(object.export().unwrap()).await.unwrap();
        (id, object)
    }

    #[tokio::test]
    async fn evicts_oldest_cached_until_under_cap() {
        let _guard = LOCK.lock().unwrap();
        let _ = scratch_home();
        let runtime = Runtime::open().await.unwrap();
        let node = Node::new(runtime);
        let runtime = node.runtime.clone();

        // Small cap: 8 KiB. Two cached objects at 64 KiB each + one owned.
        unsafe {
            std::env::set_var("CANOPEE_CACHE_MAX_MB", "0");
        }
        let (old_id, _) = imported_object(&runtime, &vec![0u8; 64 * 1024]).await;
        let (new_id, _) = imported_object(&runtime, &vec![1u8; 64 * 1024]).await;
        let owned = runtime.put_object(vec![2u8; 64 * 1024], ObjectType::Blob, None).await.unwrap();

        assert!(runtime.cached_bytes().await > 0);

        node.evict_if_over_cap(cache_cap_bytes()).await;

        // Both cached objects are evicted (each exceeds the 8 KiB cap on its
        // own), the owned object stays.
        assert!(!runtime.storage.exists(&old_id).await);
        assert!(!runtime.storage.exists(&new_id).await);
        assert!(runtime.storage.exists(&owned.id).await);
        assert!(runtime.cache.is_owned(&owned));
        assert_eq!(runtime.cached_bytes().await, 0);
    }

    #[tokio::test]
    async fn never_evicts_owned_objects() {
        let _guard = LOCK.lock().unwrap();
        let _ = scratch_home();
        let runtime = Runtime::open().await.unwrap();
        let node = Node::new(runtime);
        let runtime = node.runtime.clone();

        unsafe {
            std::env::set_var("CANOPEE_CACHE_MAX_MB", "0");
        }
        let owned = runtime.put_object(vec![3u8; 64 * 1024], ObjectType::Blob, None).await.unwrap();
        assert_eq!(runtime.cached_bytes().await, 0);

        node.evict_if_over_cap(cache_cap_bytes()).await;

        assert!(
            runtime.storage.exists(&owned.id).await,
            "eviction must never touch owned objects"
        );
    }

    #[tokio::test]
    async fn cache_cap_bytes_parses_env() {
        unsafe {
            std::env::set_var("CANOPEE_CACHE_MAX_MB", "4");
        }
        assert_eq!(cache_cap_bytes(), 4 * 1024 * 1024);
        unsafe {
            std::env::set_var("CANOPEE_CACHE_MAX_MB", "junk");
        }
        assert_eq!(cache_cap_bytes(), 256 * 1024 * 1024);
    }
}
