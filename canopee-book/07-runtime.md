# Chapter 7: The Runtime

## Tying It All Together

The runtime is the integration layer that connects identity, storage, and networking into a coherent system. It is the component that makes Canopee *work* — not just a collection of crates, but a functioning peer node.

```rust
pub struct Runtime {
    pub config: Config,
    pub identity: Arc<Identity>,   // the *person* — shared account key
    pub device_key: Arc<DeviceKey>, // the *machine* — this device's PeerId
    pub storage: Arc<Storage>,
    pub network: NetworkManager,
    pub cache: Arc<Cache>,
    state: RwLock<NodeState>,
    shared: SharedSet,
    pairing: RwLock<Option<OwnPairing>>, // in-flight LAN pairing session
}
```

## Initialization

When a runtime opens, it performs a precise sequence of operations:

### 1. Create Directory Structure

```
~/.canopee/           (user root)
├── identity/
│   ├── identity.key    (shared across the user's devices)
│   └── device.key      (this machine only)
├── storage/
├── records/
└── aliases           (JSON file)

<app root>/
├── state/
└── exports/
```

Directories are created if they don't exist. The operation is idempotent.

### 2. Identity and Device Key Provisioning

```rust
Identity::create_if_absent(&config.identity_path().join("identity.key"))
DeviceKey::load_or_create(&config.device_key_path(), &default_device_name)
```

If no account key exists, one is generated. If one exists, it is loaded. The `create_if_absent` method is race-safe — two processes starting simultaneously on the same user root won't create two keys. The device key is the *machine's* keypair: also race-safe, but distinct on every device and never shared — its public key is the libp2p `PeerId` the swarm uses, so several devices of one identity can be online at once (see Chapter 3).

### 3. Load or Create NodeState

The runtime loads `NodeState` from `state/node.state`, creating it if absent (the load-or-create logic is inline in `Runtime::open_with_config`; there is no separate `NodeState::load_or_create` constructor).

`NodeState` tracks:
- The node's identity
- When it was created
- When it was last started
- Whether it's currently running
- The software version

State is persisted atomically at `state/node.state`.

### 4. Open Storage

```rust
Storage::new(storage_path)
```

Opens the flat-file object store. No indexing, no database — just a directory of content-addressed files.

### 5. Start the Network

```rust
NetworkManager::new(
    device_key.keypair(),      // the swarm's PeerId comes from the device key
    listen_addr,
    object_provider,
    config.mdns_enabled(),
)
```

The network manager builds the libp2p swarm from the *device* keypair, dials the bootstrap addresses, begins mDNS discovery, and prepares to serve objects. This happens before loading sharing state, since the DHT re-announce needs a running network.

### 6. Load Sharing State

The runtime loads the persisted `HomeIndex` and re-announces all `shared: true` objects to the DHT. This ensures sharing survives restarts.

## Constructors

The runtime provides several constructors for different deployment scenarios:

### Isolated Mode

```rust
Runtime::open_with_root(dir)
```

Everything under one directory. For testing or fully isolated deployments.

### Embedded App Mode

```rust
Runtime::open_with_user_root(app_dir)
```

App state under `app_dir`, shared user data under `~/.canopee`. This is the mode used by desktop applications.

### Config Mode

```rust
Runtime::open_with_config(config)
```

Full control over both roots. For testing with scratch directories.

## The Object Lifecycle in the Runtime

### Putting an Object

```
Application → Runtime::put(payload) → Storage::put_verified() → Object on disk
```

1. The application provides raw payload bytes
2. The runtime wraps it in an `ObjectPayload` with the owner's identity and metadata
3. The runtime computes the `ObjectId` (SHA-256 of bincode(payload))
4. The runtime signs the payload with the owner's private key
5. The complete `Object` is verified and written to disk

### Getting an Object

```
Application → Runtime::get(id) → Storage::get_verified() → Object (or None)
```

1. The runtime requests the object by ID from storage
2. Storage reads the file and deserializes it
3. The runtime verifies the content hash and signature
4. Returns the verified object (or `None` if verification fails)

### Sharing an Object

```
Runtime::share_object(name, object_id, app)
  → Add HomeEntry with shared=true
  → Save HomeIndex
  → Announce to DHT
  → Update serving set
```

### Fetching from Peers

```
Runtime::fetch_object(object_id, from: Option<PeerId>)
  → NetworkManager::get_object(peer_id, object_id)
  → Receive ExportBundle
  → Verify signature and hash (via import)
  → Import to local storage
```

`Runtime::fetch_object` always imports after fetching. At the lower network layer, `NetworkManager::get_object` returns the raw `ExportBundle` without importing — you can use that to inspect an object without storing it — but the high-level runtime method combines both steps.

## User Record Operations

The runtime owns the entire user record lifecycle:

### Publishing

```rust
runtime.publish_pointer(name, object_id)
```

Creates a signed record pointing to the given object, saves it locally, publishes to DHT. The owner is always the runtime's own identity.

### Resolution

```rust
runtime.resolve_pointer(owner, name).await → Result<Option<AppPointerRecord>>
```

Checks local cache first, then DHT. Verifies the record's signature before returning. Note it returns the record envelope (containing the target `ObjectId`); you then fetch the referenced object separately.

### Profile Management

```rust
runtime.save_profile(profile)
runtime.load_profile().await → Result<Option<Profile>>   // own profile
runtime.resolve_profile(owner).await → Result<Option<Profile>>  // any peer's
```

### Contact Management

```rust
runtime.save_contact_list(contacts)
runtime.load_contact_list().await → Result<Option<ContactList>>
```

### Home Index Management

```rust
runtime.save_home_index(index)
runtime.load_home_index().await → Result<Option<HomeIndex>>
```

## Username System

Usernames provide human-readable aliases for cryptographic identities:

```rust
runtime.claim_username("alice")
runtime.resolve_username(&owner).await → Option<UsernameRecord>   // by owner
runtime.resolve_owner_from_username("alice").await → Option<IdentityId>  // lookup
```

(The "show my username" operation is exposed on the SDK client as `client.show_username()`, which resolves the username record of the node's own identity.)

### How Usernames Work

1. **Claiming**: creates a `UsernameRecord { username, version }` signed by the owner, pointed to by `(owner, "username")`
2. **Registry**: a separate record `username:<lowercased>` maps to the canonical owner, allowing lookup by name
3. **Resolution**: derives the registry key, fetches the record, verifies the signature, and returns the owner's `IdentityId`
4. **Normalization**: usernames are normalized to `[a-z0-9._-]` and stored lowercase

Username resolution is spoof-verified and fails closed — if the record is invalid, resolution returns `None`.

## Peer Enrichment

When the runtime exposes peer information (for the CLI `peers` command or the chat app), it enriches each connected peer with:

1. Their claimed username (if any)
2. Their profile display name (if any)

This gives a human-readable view of the network:

```
Peer 12D3KooWABCD... (alice, Alice Smith)
Peer 12D3KooWXY... (bob, Bob Jones)
```

## Import Safety

The runtime refuses to import an object if its ID already exists in the store:

```rust
runtime.import(bundle)  // fails if object ID already present
```

For content-addressed IDs, this only triggers on byte-identical duplicates. The error is a clear `bail!` — no silent overwrites.

## The Runtime as Application Foundation

The runtime is the foundation for building applications. It provides:

- **Identity management**: create, load, encrypt
- **Object storage**: put, get, list, export, import
- **Networking**: discover, connect, exchange
- **Records**: publish, resolve, share
- **User data**: profiles, contacts, home indexes, usernames

Applications layer their specific logic on top. The chat app adds encryption and conversation management. The app hosting system adds manifests and HTTP serving. But the runtime provides the building blocks for all of them.
