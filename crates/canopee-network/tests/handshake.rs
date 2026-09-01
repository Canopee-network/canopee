use canopee_identity::Identity;
use canopee_network::{Multiaddr, NetworkManager, ObjectProvider};
use canopee_storage::{Export, ExportBundle, Object, ObjectId, ObjectType};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Creates a fresh identity key under a scratch directory (created if
/// missing), mirroring how `Runtime::open` provisions its identity file.
async fn test_identity(name: &str) -> Arc<Identity> {
    let dir = std::env::temp_dir().join(format!("canopee_net_test_{}", std::process::id()));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    Arc::new(Identity::create(dir.join(name).to_str().unwrap()).await.unwrap())
}

/// Starts a manager on an ephemeral loopback port and returns the manager
/// together with its actual bound address (for dialing).
async fn make_manager(
    identity: Arc<Identity>,
    provider: Arc<dyn ObjectProvider>,
) -> (NetworkManager, Multiaddr) {
    let listen: Multiaddr = "/ip4/127.0.0.1/tcp/0".parse().unwrap();
    let manager = NetworkManager::new(identity, listen, provider, true).unwrap();
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
    let identity_a = test_identity("a.key").await;
    let identity_b = test_identity("b.key").await;
    let peer_b_id = libp2p::PeerId::from(identity_b.as_ref().keypair().public());

    let (manager_a, _addr_a) = make_manager(
        identity_a,
        Arc::new(NoObjects),
    )
    .await;
    let (_manager_b, addr_b) = make_manager(identity_b, Arc::new(NoObjects)).await;

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
    let identity_a = test_identity("c.key").await;
    let identity_b = test_identity("d.key").await;

    let object = Object::new(&identity_b, b"hello canopee".to_vec(), ObjectType::Blob);
    let object_id = object.id.clone();
    let bundle = object.export().unwrap();

    let mut objects = HashMap::new();
    objects.insert(object_id.clone(), bundle);
    let provider_b = Arc::new(InMemoryObjects(Mutex::new(objects)));

    let (manager_a, _addr_a) = make_manager(identity_a, Arc::new(NoObjects)).await;
    let (manager_b, addr_b) = make_manager(identity_b, provider_b).await;

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
    let identity_a = test_identity("e.key").await;
    let identity_b = test_identity("f.key").await;

    let (manager_a, _addr_a) = make_manager(identity_a, Arc::new(NoObjects)).await;
    let (manager_b, addr_b) = make_manager(identity_b, Arc::new(NoObjects)).await;

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