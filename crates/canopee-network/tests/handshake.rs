use canopee_network::{Multiaddr, NetworkManager, ObjectProvider, ObjectStore};
use canopee_storage::{Export, ExportBundle, Object, ObjectId, ObjectType, Storage, Verify};
use libp2p::identity::Keypair;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A fresh device keypair — the network identity of the machine, mirroring how
/// `Runtime::open` provisions a `DeviceKey` per user root.
fn device_keypair() -> Keypair {
    Keypair::generate_ed25519()
}

/// A store that refuses every push, like an edge/relay that hosts no content
/// of its own (the Store arm must answer `StoreFailed`, not hang or crash).
struct RejectStore;

#[async_trait::async_trait]
impl ObjectStore for RejectStore {
    async fn put_verified(&self, _object: &canopee_storage::Object) -> anyhow::Result<()> {
        anyhow::bail!("this node stores no objects")
    }
}

/// Starts a manager on an ephemeral loopback port and returns the manager
/// together with its actual bound address (for dialing).
async fn make_manager(
    keypair: Keypair,
    provider: Arc<dyn ObjectProvider>,
    store: Arc<dyn ObjectStore>,
) -> (NetworkManager, Multiaddr) {
    let listen: Multiaddr = "/ip4/127.0.0.1/tcp/0".parse().unwrap();
    let manager = NetworkManager::new(keypair, listen, provider, store, true).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let bound = loop {
        let addrs = manager.listen_addresses().await.unwrap();
        if let Some(a) = addrs
            .iter()
            .find(|a| a.to_string().contains("/tcp/"))
            .cloned()
        {
            break a;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for a bound TCP address"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    (manager, bound)
}

/// A fresh identity key under a scratch directory (created if missing),
/// mirroring how `Runtime::open` provisions its identity file. Used only to
/// sign objects; the swarm runs on the separate device keypairs below.
async fn test_identity(name: &str) -> canopee_identity::Identity {
    let dir = std::env::temp_dir().join(format!("canopee_net_test_{}", std::process::id()));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    canopee_identity::Identity::create(dir.join(name).to_str().unwrap())
        .await
        .unwrap()
}

struct NoObjects;

#[async_trait::async_trait]
impl ObjectProvider for NoObjects {
    async fn get_object(&self, _id: &ObjectId) -> Option<ExportBundle> {
        None
    }
}

struct InMemoryObjects(Mutex<HashMap<ObjectId, ExportBundle>>);

#[async_trait::async_trait]
impl ObjectProvider for InMemoryObjects {
    async fn get_object(&self, id: &ObjectId) -> Option<ExportBundle> {
        self.0.lock().unwrap().get(id).cloned()
    }
}

#[tokio::test]
async fn two_nodes_dial_and_discover_via_kad() {
    let device_a = device_keypair();
    let device_b = device_keypair();
    let peer_b_id = libp2p::PeerId::from(device_b.public());

    let (manager_a, _addr_a) =
        make_manager(device_a, Arc::new(NoObjects), Arc::new(RejectStore)).await;
    let (_manager_b, addr_b) =
        make_manager(device_b, Arc::new(NoObjects), Arc::new(RejectStore)).await;

    manager_a.dial(addr_b.clone()).await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_peer_b = false;
    while tokio::time::Instant::now() < deadline {
        if let Ok(peers) = manager_a.peers().await {
            if peers.iter().any(|p| p.peer_id == peer_b_id) {
                saw_peer_b = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        saw_peer_b,
        "node A should see node B as a peer (saw: {:?})",
        manager_a
            .peers()
            .await
            .unwrap()
            .iter()
            .map(|p| p.peer_id)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn node_fetches_object_announced_by_peer() {
    let identity_b = test_identity("c.key").await;
    let device_a = device_keypair();
    let device_b = device_keypair();

    let object = Object::new(&identity_b, b"hello canopee".to_vec(), ObjectType::Blob);
    let object_id = object.id.clone();
    let bundle = object.export().unwrap();

    let mut objects = HashMap::new();
    objects.insert(object_id.clone(), bundle);
    let provider_b = Arc::new(InMemoryObjects(Mutex::new(objects)));

    let (manager_a, _addr_a) =
        make_manager(device_a, Arc::new(NoObjects), Arc::new(RejectStore)).await;
    let (manager_b, addr_b) = make_manager(device_b, provider_b, Arc::new(RejectStore)).await;

    manager_a.dial(addr_b).await.unwrap();
    manager_b.announce(object_id.clone()).await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut providers: Vec<libp2p::PeerId> = Vec::new();
    while tokio::time::Instant::now() < deadline {
        if let Ok(p) = manager_a.find_providers(object_id.clone()).await {
            if !p.is_empty() {
                providers = p;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        !providers.is_empty(),
        "node A should find node B as a provider"
    );

    let fetched = manager_a
        .get_object(providers[0], object_id.clone())
        .await
        .unwrap();
    assert_eq!(fetched.object.id, object_id);
    assert_eq!(fetched.object.payload.data, b"hello canopee");
}

#[tokio::test]
async fn node_receives_pubsub_message_from_peer() {
    let device_a = device_keypair();
    let device_b = device_keypair();

    let (manager_a, _addr_a) =
        make_manager(device_a, Arc::new(NoObjects), Arc::new(RejectStore)).await;
    let (manager_b, addr_b) =
        make_manager(device_b, Arc::new(NoObjects), Arc::new(RejectStore)).await;

    manager_a.dial(addr_b).await.unwrap();

    let mut rx_a = manager_a.subscribe("canopee-chat").await.unwrap();
    manager_b.subscribe("canopee-chat").await.unwrap();

    // Wait for the gossipsub mesh to establish before publishing.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut sent = false;
    while tokio::time::Instant::now() < deadline {
        if manager_b
            .publish("canopee-chat", b"hello from B".to_vec())
            .await
            .is_ok()
        {
            sent = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(sent, "publish should eventually succeed");

    let message = tokio::time::timeout(Duration::from_secs(5), rx_a.recv())
        .await
        .expect("timed out waiting for pubsub message")
        .unwrap();

    assert_eq!(message.topic, "canopee-chat");
    assert_eq!(message.data, b"hello from B");
}

#[tokio::test]
async fn pushed_object_is_stored_by_connected_peer() {
    // B runs a real content-addressed store as its ObjectStore; A pushes a
    // bundle straight over the object-exchange protocol (no DHT in between),
    // and the object must land, verified, in B's store.
    let identity = test_identity("push_owner.key").await;
    let object = Object::new(
        &identity,
        b"pushed over the wire".to_vec(),
        ObjectType::Blob,
    );
    let object_id = object.id.clone();
    let bundle = object.export().unwrap();

    let store_dir =
        std::env::temp_dir().join(format!("canopee_net_push_store_{}", std::process::id()));
    let store_root = store_dir.join("store");
    tokio::fs::create_dir_all(&store_root).await.unwrap();
    let store_b = Arc::new(Storage::new(store_root.to_str().unwrap()));

    let (manager_a, _addr_a) =
        make_manager(device_keypair(), Arc::new(NoObjects), Arc::new(RejectStore)).await;
    let (manager_b, addr_b) =
        make_manager(device_keypair(), Arc::new(NoObjects), store_b.clone()).await;

    manager_a.dial(addr_b.clone()).await.unwrap();

    // Wait until A knows B as a connected peer (Store targets = connected
    // peers map).
    let peer_b_id = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut saw = None;
        while tokio::time::Instant::now() < deadline {
            if let Ok(peers) = manager_a.peers().await {
                saw = peers.first().map(|p| p.peer_id);
                if saw.is_some() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        saw.expect("A should see B connected before pushing")
    };

    // Strict push to the specific peer: must resolve Ok only once B stored it.
    manager_a
        .replicate_object(bundle.clone(), Some(peer_b_id))
        .await
        .expect("replicate_object(Some(peer)) should succeed");

    let stored = store_b
        .get_verified(&object_id)
        .await
        .expect("the receiving swarm must have stored the pushed object");
    assert!(stored.verify());
    assert_eq!(stored.payload.data, b"pushed over the wire");

    // Re-verify the interplay with GetObject: B can now hand it back.
    assert!(
        manager_a
            .get_object(peer_b_id, object_id.clone())
            .await
            .is_err(),
        "B's provider serves nothing (NoObjects), so a pull request is refused"
    );
}
