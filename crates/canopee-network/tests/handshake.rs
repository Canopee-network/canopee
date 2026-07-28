use canopee_identity::Identity;
use canopee_network::{Multiaddr, NetworkManager, ObjectProvider};
use canopee_storage::{Export, ExportBundle, Object, ObjectId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    let identity_a = Arc::new(Identity::create("/tmp/canopee_net_test/a.key").await.unwrap());
    let identity_b = Arc::new(Identity::create("/tmp/canopee_net_test/b.key").await.unwrap());

    let addr_a: Multiaddr = "/ip4/127.0.0.1/tcp/38111".parse().unwrap();
    let addr_b: Multiaddr = "/ip4/127.0.0.1/tcp/38112".parse().unwrap();

    let manager_a = NetworkManager::new(identity_a, addr_a.clone(), Arc::new(NoObjects)).unwrap();
    let _manager_b =
        NetworkManager::new(identity_b, addr_b.clone(), Arc::new(NoObjects)).unwrap();

    manager_a.dial(addr_b).await.unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    let peers_a = manager_a.peers().await.unwrap();
    assert_eq!(peers_a.len(), 1, "node A should see node B as a peer");
}

#[tokio::test]
async fn node_fetches_object_announced_by_peer() {
    let identity_a = Arc::new(
        Identity::create("/tmp/canopee_net_test/c.key")
            .await
            .unwrap(),
    );
    let identity_b = Arc::new(
        Identity::create("/tmp/canopee_net_test/d.key")
            .await
            .unwrap(),
    );

    let addr_a: Multiaddr = "/ip4/127.0.0.1/tcp/38113".parse().unwrap();
    let addr_b: Multiaddr = "/ip4/127.0.0.1/tcp/38114".parse().unwrap();

    let object = Object::new(&identity_b, b"hello canopee".to_vec());
    let object_id = object.id.clone();
    let bundle = object.export().unwrap();

    let mut objects = HashMap::new();
    objects.insert(object_id.clone(), bundle);
    let provider_b = Arc::new(InMemoryObjects(Mutex::new(objects)));

    let manager_a = NetworkManager::new(identity_a, addr_a.clone(), Arc::new(NoObjects)).unwrap();
    let manager_b = NetworkManager::new(identity_b, addr_b.clone(), provider_b).unwrap();

    manager_a.dial(addr_b).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    manager_b.announce(object_id.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let providers = manager_a.find_providers(object_id.clone()).await.unwrap();
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
    let identity_a = Arc::new(
        Identity::create("/tmp/canopee_net_test/e.key")
            .await
            .unwrap(),
    );
    let identity_b = Arc::new(
        Identity::create("/tmp/canopee_net_test/f.key")
            .await
            .unwrap(),
    );

    let addr_a: Multiaddr = "/ip4/127.0.0.1/tcp/38115".parse().unwrap();
    let addr_b: Multiaddr = "/ip4/127.0.0.1/tcp/38116".parse().unwrap();

    let manager_a = NetworkManager::new(identity_a, addr_a.clone(), Arc::new(NoObjects)).unwrap();
    let manager_b = NetworkManager::new(identity_b, addr_b.clone(), Arc::new(NoObjects)).unwrap();

    manager_a.dial(addr_b).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut rx_a = manager_a.subscribe("canopee-chat").await.unwrap();
    manager_b.subscribe("canopee-chat").await.unwrap();

    // Give gossipsub time to exchange subscriptions over the mesh.
    tokio::time::sleep(Duration::from_millis(500)).await;

    manager_b
        .publish("canopee-chat", b"hello from B".to_vec())
        .await
        .unwrap();

    let message = tokio::time::timeout(Duration::from_secs(5), rx_a.recv())
        .await
        .expect("timed out waiting for pubsub message")
        .unwrap();

    assert_eq!(message.topic, "canopee-chat");
    assert_eq!(message.data, b"hello from B");
}
