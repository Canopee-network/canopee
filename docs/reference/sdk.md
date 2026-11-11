# SDK client reference

`CanopeeClient` (crates/canopee-sdk/src/client.rs): `async` Rust client for
the node. It speaks `canopee-protocol` over the node's Unix socket.

## Connect

```rust
let client = CanopeeClient::connect().await?; // default node.sock
```

## Identity & identity transfer

| Method | Notes |
|---|---|
| `identity() -> IdentityId` | `canopee://identity/<peer-id>` |
| `export_identity(passphrase) -> Vec<u8>` | encrypted transfer envelope |
| `import_identity(bytes, passphrase, overwrite)` | restart node to adopt |
| `device() -> (String, String)` | (peer id of this machine, friendly device name) |
| `device_list() -> Vec<DeviceInfo>` | every device carrying this identity |
| `resolve_owner_device(owner) -> Option<...>` | which device peer id to dial |

## Objects

| Method | Notes |
|---|---|
| `put(data, name?) -> ObjectId` | store locally, encrypted to this identity |
| `put_object(data, type, name?) -> ObjectId` | typed object, encrypted to this identity |
| `put_file(name, data) -> ObjectId` | file-typed, encrypted to this identity |
| `set_name(id, name)` | local convenience alias |
| `get(id) -> Object` | retrieve by id (verified path) |
| `list() -> Vec<ObjectInfo>` | local listing |
| `export(id) -> ExportBundle` / `import(bundle)` | portable transfer |

## Network

| Method | Notes |
|---|---|
| `peers() -> Vec<PeerInfo>` | known/connected peers |
| `dial(addr)` | dial a multiaddr |
| `listen_via_relay(relay_addr)` | become reachable via a relay |
| `relay_reservations() -> Vec<RelayReservationInfo>` | NAT/relay status |
| `announce(id)` | advertise this node as a DHT provider |
| `find_providers(id) -> Vec<String>` | who serves the object |
| `fetch_object(peer_id, id)` | direct fetch from a specific peer |

## Apps

| Method | Notes |
|---|---|
| `publish_app_pointer(name, manifest_id)` | `(owner, "app:<name>")` |
| `resolve_app_pointer(owner, name) -> ObjectId?` | find the latest manifest |

## User records & pointers

| Method | Notes |
|---|---|
| `publish_pointer(name, target)` | publish `(owner, name) → object` |
| `resolve_pointer(owner, name)` | cache → DHT → verify → fetch |
| `save_profile(profile)` / `load_profile()` | `(owner, "profile")` |
| `save_contact_list(list)` / `load_contact_list()` | `(owner, "contacts")` |
| `save_home_index(index)` / `load_home_index()` | `(owner, "home")` |
| `set_home_entry_shared(name, shared)` | share / unshare the entry |
| `share_object(id, name)` | upsert shared entry + announce provider |
| `share_object_for(name, id, app, recipients)` | publish a copy encrypted to owner + recipients |
| `unshare(name)` | withdraw announcement + undo entry; does not erase existing recipient copies |

## Usernames

| Method | Notes |
|---|---|
| `claim_username(username)` | unique, network-wide, case-insensitive |
| `show_username() -> Option<String>` | this identity's claim |
| `resolve_username(username) -> IdentityId` | reverse-resolve via DHT |

## Pairing

| Method | Notes |
|---|---|
| `pair_initiate() -> PairingQrData` | new-device side: mint code |
| `pair_complete(qr, code) -> status` | existing-device side: approve + deliver |

## Sync

Record refresh:

| Method | Notes |
|---|---|
| `sync_with_peer(peer_id) -> SyncResult` | refresh from one peer |
| `sync_with_all_devices() -> SyncResult` | refresh from every registered device |

Folder sync:

| Method | Notes |
|---|---|
| `sync_create_root(path, name) -> SyncRootInfo` | create root + local mapping; name defaults to folder name |
| `sync_join_root(name, path) -> SyncRootInfo` | map an existing root locally; existing files are preserved |
| `sync_list_roots() -> Vec<SyncRootInfo>` | roots with local mappings and revisions |
| `sync_status(name) -> Vec<SyncRootInfo>` | one root, or all when `name` is `None` |
| `sync_now(name) -> Vec<SyncRunResult>` | one pull/push pass; all local roots when `name` is `None` |
| `sync_remove_root(name)` | drop the local mapping only |

## Capabilities

| Method | Notes |
|---|---|
| `grant_capability(subject, resource, permissions, expires_at)` | issue + record in index |
| `list_capabilities() -> Option<CapabilityIndex>` | grants, with revocation state |
| `revoke_capability(id)` | mark revoked |
| `check_capability(capability)` | verify presented bundle end to end |
| `check_access(subject, permission, resource)` | "does subject currently hold a valid grant" |

## Pub/sub

| Method | Notes |
|---|---|
| `publish(topic, data)` | gossipsub publish |
| `subscribe(topic) -> Subscription` | stream of `PubSubMessage` frames |

## Lifecycle

| Method | Notes |
|---|---|
| `shutdown()` | politely stop the node |

## Sending files

```rust
let id = client.put_file("photo.jpg", bytes).await?;
client.share_object(id, "photo").await?;
// peers can now: fetch_object or fetch by name "photo"
```

## Real-time messages

```rust
let sub = client.subscribe("room-42").await?;
client.publish("room-42", b"hello".to_vec()).await?;
tokio::pin!(sub);
while let Some(msg) = sub.next().await {
    use_msg(msg); // PubSubMessage
}
```

The full set of wire types the client maps onto is in
[Protocols & DHT records](protocols.md). The
[SDK app guides](../guides/README.md) show the client in action.