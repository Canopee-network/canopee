use canopee_node::Node;
use canopee_sdk::{
    CanopeeClient, Contact, ContactList, HomeEntry, HomeIndex, IdentityId, ObjectType, Permission,
    Profile, Resource,
};

/// Both integration tests in this binary drive a real in-process node whose
/// config is rooted at the *process-global* `$HOME`. They must not run
/// concurrently, or whichever one sets `HOME` last configures the node the
/// other is about to spawn. Serialize them with a shared mutex.
static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Spins up a real node (in-process) bound to a scratch `$HOME`, then drives
/// it purely through `CanopeeClient`, the same way an app would.
#[tokio::test]
async fn app_uses_identity_storage_and_network_via_sdk() {
    let _guard = HOME_LOCK.lock().unwrap();
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
    let id = client.put(b"hello from an app".to_vec(), None).await.unwrap();
    let object = client.get(id.clone()).await.unwrap();
    assert_eq!(object.payload.data, b"hello from an app");

    let objects = client.list().await.unwrap();
    assert!(
        objects.iter().any(|o| o.id == id),
        "the stored object must be listed (the node also stores its device record object)"
    );

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

    // ---- user records over the socket ----

    // Generic pointer round-trip through the cache-aware Runtime layer.
    client.publish_pointer("app:demo", id.clone()).await.unwrap();
    let record = client
        .resolve_pointer(identity.clone(), "app:demo")
        .await
        .unwrap()
        .expect("a just-published pointer must resolve");
    assert_eq!(record.manifest, id);
    assert!(record.verify());

    // Profile: absent until saved, then round-trips with server-side
    // version bumping.
    assert!(client.load_profile().await.unwrap().is_none());
    let profile = Profile {
        display_name: "alice".into(),
        dh_public_key: [7u8; 32],
        avatar: None,
        version: 0,
    };
    client.save_profile(&profile).await.unwrap();
    let loaded = client.load_profile().await.unwrap().unwrap();
    assert_eq!(loaded.display_name, "alice");
    assert_eq!(loaded.version, 1);
    client.save_profile(&profile).await.unwrap();
    assert_eq!(client.load_profile().await.unwrap().unwrap().version, 2);

    // Contact list.
    assert!(client.load_contact_list().await.unwrap().is_none());
    let list = ContactList {
        contacts: vec![Contact {
            name: "bob".into(),
            peer_id: "12D3KooB".into(),
            dh_public_key: [9u8; 32],
            note: None,
        }],
        version: 0,
    };
    client.save_contact_list(&list).await.unwrap();
    let loaded = client.load_contact_list().await.unwrap().unwrap();
    assert_eq!(loaded.contacts[0].name, "bob");
    assert_eq!(loaded.version, 1);

    // Home index + the explicit share action.
    let pic = client
        .put_object(b"png bytes".to_vec(), ObjectType::Blob, None)
        .await
        .unwrap();
    let index = HomeIndex {
        profile: None,
        contacts: None,
        entries: vec![HomeEntry {
            name: "pic.png".into(),
            object: pic.clone(),
            object_type: ObjectType::Blob,
            shared: false,
            app: Some("sdk-test".into()),
        }],
        version: 0,
    };
    client.save_home_index(&index).await.unwrap();
    let loaded = client.load_home_index().await.unwrap().unwrap();
    assert_eq!(loaded.entries.len(), 1);
    assert!(!loaded.entries[0].shared);

    client.set_home_entry_shared("pic.png", true).await.unwrap();
    let loaded = client.load_home_index().await.unwrap().unwrap();
    assert!(loaded.entries[0].shared);
    assert!(
        client.set_home_entry_shared("missing", true).await.is_err(),
        "sharing an unknown entry must fail"
    );

    let _ = client.shutdown().await;
}

/// A second rate: grants issued via the SDK verify end to end, list, gate
/// access, and deactivate on revocation — the vision §5.5 lifecycle without
/// any central authorization server.
#[tokio::test]
async fn capabilities_grant_list_check_revoke() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = std::env::temp_dir().join(format!("canopee_sdk_cap_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: single test in this binary; no other thread reads HOME concurrently.
    unsafe { std::env::set_var("HOME", &home) };

    let node = Node::open().await.unwrap();
    let node_for_task = node.clone();
    tokio::spawn(async move {
        let _ = node_for_task.run().await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let client = CanopeeClient::connect().await.unwrap();
    let identity = client.identity().await.unwrap();

    let resource = Resource::Channel("team-news".into());
    let bob = IdentityId::new("canopee://identity/bob");

    // Nothing issued yet.
    assert!(client.list_capabilities().await.unwrap().is_none());
    assert!(!client
        .check_access(&bob, Permission::Read, &resource)
        .await
        .unwrap());

    // Grant read+write on the channel to bob, never expiring.
    let cap = client
        .grant_capability(
            bob.clone(),
            resource.clone(),
            vec![Permission::Read, Permission::Write],
            None,
        )
        .await
        .unwrap();
    assert!(cap.verify());
    assert_eq!(cap.issuer, identity);

    // Listed.
    let index = client.list_capabilities().await.unwrap().unwrap();
    assert_eq!(index.entries.len(), 1);
    assert!(!index.entries[0].revoked);

    // Presented bundle verifies.
    let (valid, reason) = client.check_capability(&cap).await.unwrap();
    assert!(valid, "fresh grant must check valid: {reason}");

    // Issuer-side authorization: bob may read and write, may not publish.
    assert!(client
        .check_access(&bob, Permission::Read, &resource)
        .await
        .unwrap());
    assert!(client
        .check_access(&bob, Permission::Write, &resource)
        .await
        .unwrap());
    assert!(!client
        .check_access(&bob, Permission::Publish, &resource)
        .await
        .unwrap());
    // Someone else, or another resource: denied.
    assert!(!client
        .check_access(&IdentityId::new("canopee://identity/mallory"), Permission::Read, &resource)
        .await
        .unwrap());
    assert!(!client
        .check_access(&bob, Permission::Read, &Resource::Channel("other".into()))
        .await
        .unwrap());

    // Revoke: the grant deactivates for access checks and bundle checks alike.
    client.revoke_capability(&cap.id).await.unwrap();
    let index = client.list_capabilities().await.unwrap().unwrap();
    assert!(index.entries[0].revoked);
    assert!(!client
        .check_access(&bob, Permission::Read, &resource)
        .await
        .unwrap());
    let (valid, reason) = client.check_capability(&cap).await.unwrap();
    assert!(!valid, "revoked grant must not check valid: {reason}");
    assert!(reason.contains("revoked"));

    let _ = client.shutdown().await;
}
