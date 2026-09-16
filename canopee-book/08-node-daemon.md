# Chapter 8: The Node Daemon

## Architecture

The node is a Unix domain socket daemon that wraps the runtime. It accepts commands from clients (CLI, SDK, gateway) and executes them on behalf of the connected application.

```
Client (CLI/SDK/Gateway) → Unix Socket → Node Daemon → Runtime → Storage/Network
```

The node is a thin layer. It doesn't add business logic — it provides connection management and the wire protocol. Connections are not serialized; every accepted connection is handled concurrently in its own task.

## Starting the Node

The node binds a Unix domain socket at `{app_root}/node.sock`:

1. Remove any stale socket file from a previous run
2. Bind a new `UnixListener`
3. Mark the node as started in `NodeState`
4. Accept connections in a loop

Each connection is handled in its own tokio task, managed by a `JoinSet`. Multiple clients can connect simultaneously — the streaming `Subscribe` command doesn't block other clients.

## The Wire Protocol

Communication uses bincode serialization with a u32 length prefix:

```
[length: u32 BE][payload: bincode(NodeCommand) or bincode(NodeResponse)]
```

The length prefix is big-endian (tokio's `read_u32`/`write_u32`). This is implemented independently in both the node and the client. The protocol is simple, efficient, and type-safe.

### Commands and Responses

There are 44 commands. The table below lists each `NodeCommand` variant and its `NodeResponse`:

| Command | Response | Description |
|---------|----------|-------------|
| `Identity` | `Identity{identity_id}` | Get the node's identity |
| `Put(bytes)` | `ObjectCreated{id}` | Create and store an object |
| `PutObject{data, object_type}` | `ObjectCreated{id}` | Store raw bytes with a type tag (node builds the Object) |
| `Get(id)` | `Object{object}` | Retrieve an object |
| `List` | `Objects{objects}` | List all objects |
| `Export(id)` | `Exported{bundle}` | Export an object as bundle |
| `Import(bundle)` | `Imported` | Import a bundle |
| `Status` | `Status{identity, objects, peers}` | Node status info |
| `Shutdown` | `ShutdownAccepted` | Shut down the node |
| `Dial(addr)` | `Dialed` | Connect to a peer |
| `ListenViaRelay(addr)` | `ListeningViaRelay` | Use a relay |
| `RelayReservations` | `Peers{peers}` | List active relay reservations |
| `Publish(topic, data)` | `Published` | Pub/sub publish |
| `Subscribe(topic)` | `Subscribed` | Subscribe (streaming) |
| `Peers` | `Peers{peers}` | List connected peers |
| `FindProviders(id)` | `Providers{peer_ids}` | DHT provider lookup |
| `FetchObject(peer, id)` | `Exported{bundle}` | Fetch from peer (returns raw bundle) |
| `Announce(id)` | `Announced` | DHT announce |
| `PublishAppPointer(name, manifest)` | `AppPointerPublished` | Publish app pointer |
| `ResolveAppPointer(owner, name)` | `AppPointer{record}` | Resolve app pointer |
| `PublishPointer{name, target}` | `PointerPublished` | Publish record |
| `ResolvePointer(owner, name)` | `Pointer{record}` | Resolve record |
| `SaveProfile(profile)` | `ProfileSaved{id}` | Save profile |
| `LoadProfile` | `Profile{profile}` | Load own profile |
| `SaveContactList(contacts)` | `ContactListSaved{id}` | Save contacts |
| `LoadContactList` | `ContactList{list}` | Load own contacts |
| `SaveHomeIndex(index)` | `HomeIndexSaved{id}` | Save home index |
| `LoadHomeIndex` | `HomeIndex{index}` | Load own home index |
| `SetHomeEntryShared{name, shared}` | `HomeIndexSaved{id}` | Toggle sharing |
| `ShareObject(name, obj, app)` | `HomeIndexSaved{id}` | Share an object |
| `ClaimUsername{username}` | `UsernameClaimed` | Claim a username |
| `ResolveUsername{username}` | `UsernameOwner{owner}` | Resolve username → owner |
| `ShowUsername` | `Username{username}` | Show own username |
| `ExportIdentity(passphrase)` | `IdentityExported{bytes}` | Export the identity as an encrypted envelope — the "move to another device" action |
| `ImportIdentity(bytes, passphrase, overwrite)` | `IdentityImported{identity_id}` | Import an exported identity; takes effect after a node restart |
| `Device` | `Device{peer_id, device_name}` | This device's network peer id + name (from its per-device key) |
| `DeviceList` | `DeviceList{devices}` | List the devices currently carrying this identity |
| `ResolveOwnerDevice(owner)` | `OwnerDevice{peer_id}` | Resolve which device peer id to dial to reach `owner` |
| `AddDevice(device_id, device_name)` | `DeviceAdded` | Record a device against this identity's `devices` list |
| `RemoveDevice(device_id)` | `DeviceRemoved` | Remove a device from the `devices` list |
| `InitiatePairing` | `PairingQr{qr}` | Start a pairing session on the new device (mints a code + QR data) |
| `CompletePairing(qr, code)` | `PairingComplete{message}` | Complete pairing: verify the code, encrypt identity + records, dial and deliver |
| `SyncFromPeer(peer_id)` | `SyncComplete{result}` | Refresh user records (profile, contacts, devices) from a peer |
| `SyncDeviceList` | `SyncComplete{result}` | Refresh user records from every device in the `devices` list |

### The Subscribe Command

`Subscribe` is the one streaming command. After the initial `Subscribed` response frame, the node pushes `PubSub` frames containing published messages until the client disconnects.

```
Client → Node: Subscribe { topic }
Node → Client: Subscribed
Node → Client: PubSub { topic, source, data }  (repeated)
Node → Client: PubSub { topic, source, data }
... (until client disconnects)
```

There is no `Unsubscribe` command. Closing the socket is the unsubscribe mechanism. This is a deliberate protocol asymmetry — the server pushes, the client decides when to stop.

## The Signing Rule

**Clients never construct signed pointers or records.** This is a fundamental security property:

- `PublishAppPointer` takes a name + manifest ID. The node signs it.
- `SaveProfile` takes raw profile data. The node wraps, signs, and version-bumps it.
- `SaveContactList` takes raw contact data. The node wraps, signs, and version-bumps it.
- `SaveHomeIndex` takes raw index data. The node wraps, signs, and version-bumps it.

The node is the only entity that holds the private key. This means:

- No client code can forge a signature
- No intercepted message can be replayed as a new version
- Record authorship is unspoofable from outside the node

## Shutdown Protocol

When the node receives a `Shutdown` command:

1. The response (`ShutdownAccepted`) is written to the socket *first*
2. Then the shutdown signal is broadcast to the swarm
3. The client reliably receives the response before the node tears down

This ordering is critical — it ensures the client knows the shutdown was accepted, even though the node is about to die.

## Error Handling

All errors are represented by a single `NodeResponse::Error { message: String }` variant. The error message is human-readable and logged via `println!`. There is no structured logging — this is a developer tool, and the output is designed for direct inspection.

## Testing the Node

The node can be tested without a socket:

```rust
#[tokio::test]
async fn test_node_put() {
    let runtime = test_runtime();
    let node = Node::new(runtime);
    let response = node.handle(NodeCommand::Put(bytes)).await;
    // ... assert response
}
```

The `Node::handle` method is public and takes `&self`, bypassing the socket layer. This makes unit testing straightforward.

## Deployment

The node is deployed as a standalone binary. The `deploy/` directory includes a systemd unit file:

```ini
[Service]
ExecStart=/home/canopee/canopee/target/release/canopee-node
Environment=CANOPEE_LISTEN_PORT=4001
Restart=on-failure
```

The `start` command in the CLI currently spawns `cargo run -p canopee-node` as a detached process. For production deployments, the compiled binary should be used directly.

## Client Connection

Clients connect to the node via the Unix socket:

```rust
let client = CanopeeClient::connect().await?;
let identity = client.identity().await?;
client.put(b"hello world".to_vec()).await?;
```

The SDK (`CanopeeClient`) provides a typed, ergonomic interface over the raw protocol. `connect()` takes no arguments — it locates the socket via the default config and fails fast if the node isn't running. Each method sends one command and waits for one response, except `subscribe` which opens a persistent streaming connection.
