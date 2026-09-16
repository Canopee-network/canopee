# Chapter 9: The SDK

## Application Interface

The SDK (`CanopeeClient`) is the primary interface for applications to interact with Canopee. It provides a typed, ergonomic API over the Unix socket protocol.

```rust
let client = CanopeeClient::connect().await?;
```

`connect()` takes no arguments — it locates the node socket via the default config. The client checks if the node is running before connecting. If the node isn't up, it fails fast with a clear error. All SDK methods are async.

## Storage Operations

### Put: Create an Object

```rust
let object_id = client.put(b"my file contents".to_vec()).await?;
println!("Created: {}", object_id);
```

Takes raw bytes (`Vec<u8>`), returns the `ObjectId`. The node handles metadata, signing, and storage.

### PutObject: Store Raw Bytes with a Type Tag

```rust
let object_id = client.put_object(data, ObjectType::Blob).await?;
```

Takes raw bytes plus an `ObjectType` tag. The node constructs and signs the `Object`. You cannot pass a pre-built `Object` — the node always builds and signs it.

### Get: Retrieve an Object

```rust
let object = client.get(object_id).await?;  // returns the full Object
println!("Object: {} bytes", object.payload.data.len());
```

`get` returns the full `Object` on success; a missing object comes back as an `Err` (the node responds with `NodeResponse::Error`), not an `Option`.

### List: Browse Objects

```rust
let objects = client.list().await?;
for obj in objects {
    // ObjectInfo has: id, owner, size, verified
    println!("{}: {} bytes (verified={})", obj.id, obj.size, obj.verified);
}
```

### Export: Create a Portable Bundle

```rust
let bundle = client.export(&object_id)?;
// bundle is an ExportBundle, serializable to a .canopee file
```

### Import: Receive a Bundle

```rust
client.import(bundle).await?;
```

Refuses to import if the object ID already exists (content-addressed deduplication). Takes the bundle by value.

## Network Operations

### Peer Discovery

```rust
let peers = client.peers().await?;
for peer in peers {
    // PeerInfo has: peer_id, identity, username, display_name, addresses
    println!("{} ({:?})", peer.peer_id, peer.addresses);
}
```

### Connecting to Peers

```rust
client.dial("/ip4/192.168.1.100/tcp/4001/p2p/12D3KooWABCD...").await?;
```

### Relay Connection

```rust
client.listen_via_relay("/ip4/relay.example.com/tcp/4001/p2p/12D3KooWRELAY...").await?;
```

### DHT Operations

```rust
// Announce an object as available from this peer
client.announce(object_id).await?;

// Find who has an object
let providers = client.find_providers(object_id).await?;
```

### Fetch from Peer

```rust
let bundle = client.fetch_object(peer_id, object_id).await?;
// Note: fetch does NOT auto-import. Use import() separately.
```

This separation is intentional. Fetching is inspecting; importing is adopting. You might fetch an object to verify it before deciding to store it.

### App Manifests

```rust
// Publish a new version
client.publish_app_pointer("my-app", manifest_id).await?;

// Resolve the latest version (returns the manifest ObjectId)
let manifest_id = client.resolve_app_pointer(owner, "my-app").await?;
```

## User Record Operations

### Profile

```rust
client.save_profile(&Profile {
    display_name: "Alice".to_string(),
    dh_public_key: my_dh_key,   // [u8; 32]
    avatar: None,
    version: 1,
}).await?;

let profile = client.load_profile().await?;  // own profile, no argument
```

### Contacts

```rust
client.save_contact_list(&ContactList {
    contacts: vec![Contact {
        name: "Bob".to_string(),
        peer_id: "12D3KooWBOB...".to_string(),
        dh_public_key: bob_dh_key,              // [u8; 32]
        note: Some("Met at conference".to_string()),
    }],
    version: 1,
}).await?;
```

### Home Index

```rust
client.save_home_index(&HomeIndex {
    profile: Some(profile_id),
    contacts: Some(contacts_id),
    entries: vec![HomeEntry {
        name: "My Photo".to_string(),
        object: photo_id,
        object_type: ObjectType::Blob,
        shared: true,
        app: Some("gallery".to_string()),
    }],
    version: 1,
}).await?;
```

### Sharing

```rust
// Share an object through the home index
client.share_object("My Photo", photo_id, Some("gallery".to_string())).await?;

// Toggle sharing on an existing entry (by entry name)
client.set_home_entry_shared("My Photo", true).await?;
```

## Username Operations

```rust
// Claim a username
client.claim_username("alice").await?;

// Show my username
let username = client.show_username().await?;

// Look up someone else's username
let identity = client.resolve_username("bob").await?;
```

## Pub/Sub

### Publishing

```rust
client.publish("chat-room", b"Hello, everyone!".to_vec()).await?;
```

### Subscribing

```rust
let mut subscription = client.subscribe("chat-room").await?;

while let Some(message) = subscription.next().await? {
    // PubSubMessage has: topic, source: Option<String>, data
    println!("{:?}: {}", message.source, String::from_utf8_lossy(&message.data));
}
```

The subscription holds a persistent connection to the node. Dropping the `Subscription` object unsubscribes (closes the socket).

## The Subscription Mechanism

Under the hood, `subscribe()`:

1. Opens a new Unix socket connection to the node
2. Sends the `Subscribe { topic }` command
3. Reads the initial `Subscribed` frame
4. Returns a `Subscription` object with a `next()` method

Each call to `next()` reads one `PubSub` frame from the socket. When the socket closes (either by dropping the `Subscription` or by the node shutting down), the stream ends.

## Error Handling

All SDK methods return `anyhow::Result<T>`. Errors can come from:

- Socket communication failures
- Node-side errors (returned as `NodeResponse::Error`)
- Deserialization failures
- Connection refused (node not running)

The `unexpected()` helper provides consistent error formatting for unexpected response types.

## Testing with the SDK

The SDK includes integration tests that run a real node in-process:

```rust
#[tokio::test]
async fn test_client_lifecycle() {
    // Set up scratch directory
    let dir = tempdir().unwrap();
    unsafe { env::set_var("HOME", dir.path()) };

    // Start node in-process
    let config = Config::with_root(dir.path().to_path_buf());
    let runtime = Runtime::open_with_config(config).await.unwrap();
    let node = Node::new(runtime);
    tokio::spawn(node.run());

    // Connect client (no arguments)
    let client = CanopeeClient::connect().await.unwrap();

    // Test operations
    let id = client.put(b"test data".to_vec()).await.unwrap();
    assert!(!id.0.is_empty());
}
```

This pattern — scratch directory, in-process node, SDK client — is the standard way to test Canopee applications.
