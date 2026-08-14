use canopee_node::Node;
use canopee_sdk::CanopeeClient;

/// Spins up a real node (in-process) bound to a scratch `$HOME`, then drives
/// it purely through `CanopeeClient`, the same way an app would.
#[tokio::test]
async fn app_uses_identity_storage_and_network_via_sdk() {
    let home = std::env::temp_dir().join(format!("canopee_sdk_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: this test binary runs this single test; no other thread reads HOME concurrently.
    unsafe { std::env::set_var("HOME", &home) };

    let node = Node::open().await.unwrap();
    let node_for_task = node.clone();
    tokio::spawn(async move {
        let _ = node_for_task.run().await;
    });

    // Give the Unix socket a moment to bind.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let client = CanopeeClient::connect().await.unwrap();

    // identity
    let identity = client.identity().await.unwrap();
    assert!(identity.to_string().starts_with("canopee://identity/"));

    // storage
    let id = client.put(b"hello from an app".to_vec()).await.unwrap();
    let object = client.get(id.clone()).await.unwrap();
    assert_eq!(object.payload.data, b"hello from an app");

    let objects = client.list().await.unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].id, id);

    let bundle = client.export(id.clone()).await.unwrap();
    assert_eq!(bundle.object.id, id);

    // network: no peers connected, but the SDK <-> node wiring should still
    // behave sensibly rather than hang or panic.
    let peers = client.peers().await.unwrap();
    assert!(peers.is_empty());

    client.announce(id.clone()).await.unwrap();

    let publish_result = client.publish("app-topic", b"ping".to_vec()).await;
    assert!(
        publish_result.is_err(),
        "publishing with zero subscribed peers should fail cleanly, not hang"
    );

    let subscription = client.subscribe("app-topic").await.unwrap();
    assert_eq!(subscription.topic(), "app-topic");
    // Nothing published on this topic reaches us before we drop the subscription.
    drop(subscription);

    let _ = client.shutdown().await;
}
