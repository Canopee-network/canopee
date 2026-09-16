# Chapter 15: Building Applications on Canopee

## Architecture Patterns

Canopee supports two primary patterns for building applications:

### Pattern 1: Embedded Runtime (Recommended)

The application embeds a Canopee `Runtime` directly in its process. There is no separate node daemon, no Unix socket, no inter-process communication.

```
Application Process
├── Application Logic (Tauri, native UI, etc.)
└── Canopee Runtime (in-process)
    ├── Identity
    ├── Storage
    └── Network (libp2p swarm)
```

This is the pattern demonstrated by the chat application. The runtime runs in Tauri's managed state, with the app's data directory as the app root and `~/.canopee` as the shared user root.

**Advantages**:
- No daemon management
- No IPC overhead
- No socket permissions to manage
- Single process lifecycle

**Setup**:
```rust
let config = Config::new()
    .with_app_root(app_data_dir)
    .with_user_root(user_root)
    .with_mdns(false);  // critical for embedded use

let runtime = Runtime::open_with_config(config).await?;
```

### Pattern 2: Client-Server (CLI/SDK)

The application connects to a running Canopee node via the Unix socket using the SDK:

```
Application Process → Unix Socket → Node Process → Runtime
```

This is the pattern used by the CLI and any external tool that talks to Canopee.

**Advantages**:
- Multiple applications share one node
- Node lifecycle managed independently
- Standard client-server separation

**Setup**:
```rust
let client = CanopeeClient::connect().await?;  // takes no arguments
```

## The Tauri Integration

The chat application demonstrates the canonical Tauri integration:

### Backend (Rust)

```rust
use canopee_runtime::Runtime;
use std::sync::Arc;

struct AppState {
    runtime: Arc<Runtime>,
}

#[tauri::command]
async fn get_my_identity(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.runtime.identity().id().to_string())
}

#[tauri::command]
async fn send_message(
    state: State<'_, AppState>,
    name: String,
    text: String,
) -> Result<(), String> {
    let conversation = state.get_conversation(&name)?;
    let encrypted = conversation.encrypt(text.as_bytes());
    // `network` is a public field; publish takes (topic: &str, data: Vec<u8>)
    state.runtime.network
        .publish(conversation.topic(), encrypted)
        .await
        .map_err(|e| e.to_string())
}
```

### Frontend (JavaScript)

```javascript
const { invoke } = window.__TAURI__.tauri;
const { listen } = window.__TAURI__.event;

// Send a message (the chat app's send_message takes { name, text })
await invoke('send_message', { name, text });

// Listen for incoming messages (payload is { from, text })
listen('chat-message', (event) => {
    const { from, text } = event.payload;
    displayMessage(from, text);
});
```

### Lifecycle

1. Tauri creates the `Runtime` on app start (managed state)
2. The runtime initializes identity, storage, and network
3. The backend exposes Tauri commands that call runtime methods
4. The frontend invokes commands and listens for events
5. The runtime's network layer handles discovery and messaging

## Building a Chat Application

The chat app (`canopee-chat-test/`) is a complete reference implementation:

### Step 1: Identity and Contact Exchange

```rust
// Get my identity
let identity = runtime.identity();
let dh_key = identity.dh_public_key();

// Create a contact string for sharing
let contact = format!(
    "canopee://identity/{}#dh={}",
    identity.id(),
    hex::encode(dh_key)
);

// Parse a received contact string (returns only the DH key bytes).
// The chat app's helper is `parse_contact(s) -> Result<[u8; 32], String>`;
// the caller assembles the full Contact from the peer id + DH key.
let their_dh_key = parse_contact(input)?;
```

### Step 2: Conversation Setup

```rust
// Derive shared secret (agree takes &[u8; 32])
let shared_secret = my_identity.agree(&their_dh_key);

// Derive the conversation key (HKDF inside Conversation::new in the chat app)
let conversation = Conversation::new(my_identity.clone(), their_dh_key);

// Derive the deterministic topic (chat app: conversation_topic(ours, theirs))
let topic = conversation_topic(&my_dh_key, &their_dh_key);
```

### Step 3: Encrypted Messaging

```rust
// Encrypt and publish (encrypt/decrypt are Conversation methods)
let encrypted = conversation.encrypt(message.as_bytes());
network.publish(&topic, encrypted).await?;

// Subscribe and decrypt. subscribe returns a broadcast::Receiver; consume
// it with .recv().await (returns Result), not a next() stream.
let mut rx = network.subscribe(&topic).await?;
while let Ok(msg) = rx.recv().await {
    match conversation.decrypt(&msg.data) {
        Ok(plaintext) => println!("{}", String::from_utf8_lossy(&plaintext)),
        Err(_) => {} // not for us, skip
    }
}
```

### Step 4: Persistence

```rust
// Save contact to shared ContactList (visible to all apps).
// load_contact_list takes no argument — it loads your own list.
let mut contacts = runtime.load_contact_list().await?.unwrap_or_default();
contacts.contacts.push(new_contact);
runtime.save_contact_list(&contacts).await?;

// Save conversation metadata to HomeIndex (third arg is Option<String>)
runtime.share_object("Chat with Bob", &conversation_id, Some("chat".to_string())).await?;
```

## Building a Data-Sharing Application

For applications that share data objects (photos, documents, files):

### Publishing Data

```rust
// Create a blob object (put takes Vec<u8> by value)
let object_id = runtime.put(file_bytes).await?;

// Add to home index as shared (share_object also announces to the DHT)
runtime.share_object("My Photo", &object_id, Some("gallery".to_string())).await?;
```

### Consuming Data

```rust
// There is no resolve_home_index on Runtime. Resolve the (owner, "home")
// pointer, then fetch the referenced HomeIndex object.
if let Some(record) = runtime.resolve_pointer(&owner, "home").await? {
    let index_obj = runtime.fetch_object(record.manifest.clone(), Some(peer_id)).await?;
    let home_index: HomeIndex = index_obj.decode()?;

    for entry in &home_index.entries {
        if entry.shared {
            // network is a public field; get_object takes (PeerId, ObjectId) by value
            let bundle = runtime.network
                .get_object(peer_id, entry.object.clone())
                .await?;

            // Verify and import
            runtime.import(bundle).await?;
        }
    }
}
```

## Building a Decentralized App Store

### Publishing an App

```rust
// Walk the build directory. publish_directory lives in the CLI crate and
// takes the SDK client, returning (entrypoint, assets) — the caller then
// builds the AppManifest, stores it, and publishes the app pointer.
let (entrypoint, assets) = publish_directory(&client, Path::new("./build")).await?;

// The manifest is then resolvable via:
// (owner_id, "my-app") → AppPointer → AppManifest → assets
```

### Installing an App

```rust
// Resolve the latest version (SDK: returns the manifest ObjectId)
if let Some(manifest_id) = client.resolve_app_pointer(owner, "my-app").await? {
    // Fetch the manifest object and decode it
    let manifest_obj = client.fetch_object(provider_peer, manifest_id).await?;
    let manifest: AppManifest = manifest_obj.object.decode()?;

    // Fetch all assets
    for (path, asset_id) in &manifest.assets {
        let bundle = client.fetch_object(provider_peer.clone(), asset_id.clone()).await?;
        client.import(bundle).await?;
    }

    // Serve locally (serve takes an in-memory file map)
    // serve(files, port, open_browser).await?;
}
```

## Testing Patterns

### In-Process Testing

```rust
#[tokio::test]
async fn test_my_feature() {
    let dir = tempdir().unwrap();
    unsafe { std::env::set_var("HOME", dir.path()) };

    let config = Config::with_root(dir.path().to_path_buf());
    let runtime = Runtime::open_with_config(config).await.unwrap();

    // Test operations directly on the runtime (async, Vec<u8> by value)
    let id = runtime.put(b"test data".to_vec()).await.unwrap();
    let obj = runtime.get(&id).await.unwrap().unwrap();
    assert_eq!(obj.payload.data, b"test data");
}
```

### Multi-Peer Testing

```rust
#[tokio::test]
async fn test_two_peers() {
    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();

    let config_a = Config::with_root(dir_a.path().to_path_buf());
    let config_b = Config::with_root(dir_b.path().to_path_buf());

    let runtime_a = Runtime::open_with_config(config_a).await.unwrap();
    let runtime_b = Runtime::open_with_config(config_b).await.unwrap();

    // Connect the peers. network is a public field; the manager's method is
    // listen_addresses() (plural, async, returns Vec<Multiaddr>).
    let addrs_b = runtime_b.network.listen_addresses().await.unwrap();
    runtime_a.network.dial(addrs_b[0].to_string()).await.unwrap();

    // Wait for discovery (a test helper you write yourself; the e2e tests
    // implement wait_for_peers(runtime, count)).
    let peer_b = runtime_b.identity.keypair().public().to_peer_id();
    wait_for_peer(&runtime_a, &peer_b, Duration::from_secs(10)).await;

    // Test cross-peer operations
    // ...
}
```

## Common Pitfalls

### Forgetting mDNS Disable

Two runtimes sharing one identity on the same machine must disable mDNS:

```rust
// WRONG: two swarms announce the same PeerId on multicast
let config = Config::new().with_app_root(dir);

// RIGHT: only one swarm announces
let config = Config::new().with_app_root(dir).with_mdns(false);
```

### Fetching Without Importing

```rust
// This fetches but does NOT store the object
let bundle = client.fetch_object(peer, object_id).await?;

// You must explicitly import
client.import(bundle).await?;
```

### Not Verifying Records from DHT

```rust
// Distrust records from the DHT until verified
let record = runtime.resolve_pointer(&owner, "profile").await?;
// The runtime verifies the signature before returning
// But if you get raw DHT data, always call record.verify()
```

### Hardcoded Port Conflicts

When testing multiple nodes on the same machine, use ephemeral ports (the default):

```rust
// Let the OS assign ports
let config = Config::with_root(dir.path().to_path_buf());  // listen_addr = /ip4/0.0.0.0/tcp/0
```
