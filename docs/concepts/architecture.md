# Architecture

Canopee is a decentralized peer-to-peer network of **nodes**. Each node owns a
cryptographic identity, stores signed, content-addressed objects locally, and
discovers/connects to other nodes over libp2p. Apps talk to the local node
over a Unix socket — they never touch the network stack directly.

This page is the crate map: what each piece is, and how data flows through
them.

## The pieces

```
                          ┌──────────────┐
                          │  canopee-cli │   binary: `canopee`
                          │  canopee-node│   binary: `canopee-node` (daemon)
                          └──────┬───────┘
                                 │ owns / drives
                          ┌──────▼────────┐
                          │ canopee-runtime│  ties identity + storage + network together
                          └───┬─────┬──────┘
               ┌──────────────┘     └─────────────┐
        ┌──────▼───────┐                 ┌────────▼────────┐
        │canopee-storage│                 │ canopee-network │  libp2p swarm
        └──────┬───────┘                 └────────┬────────┘
               │                                  │
        ┌──────▼────────────┐                     │
        │  canopee-identity │ ◄───────────────────┘
        └───────────────────┘

   canopee-protocol   wire format between the node and its clients (SDK/CLI)
   canopee-config     shared filesystem paths (~/.canopee/...) + env vars
   canopee-sdk        client library for apps — talks to the node over its
                      Unix socket using canopee-protocol
   canopee-gateway    local WebSocket bridge so a browser tab can drive the
                      node through canopee-sdk
   canopee-e2e        end-to-end tests that spawn real node processes
```

| Crate | What it is |
|---|---|
| `canopee-identity` | Ed25519 account keypairs, X25519 DH keys, signing, verification, device keys, pairing crypto |
| `canopee-storage` | Content-addressed, signed `Object` store on disk; user records; capabilities; DHT record model |
| `canopee-network` | libp2p swarm: discovery (mDNS, Kademlia), DHT provider/record store, object exchange, pairing protocol, gossipsub pub/sub, relay + DCUtR, autonat |
| `canopee-config` | `Config`: the `~/.canopee` layout (identity, storage, records, cache, exports, socket) + env-var overrides |
| `canopee-runtime` | `Runtime`: ties identity + storage + network into one node runtime; implements every high-level operation (records, sharing, sync, pairing, capabilities) |
| `canopee-protocol` | The wire types (`NodeCommand` / `NodeResponse`, `PairingData`, `SyncResult`, …) shared between the node and its clients |
| `canopee-node` | The daemon: serves `canopee-protocol` over a Unix socket; also embeds a `Runtime` |
| `canopee-sdk` | `CanopeeClient`: the client library apps use — connect, identity, put/get, peers, publish/subscribe, capabilities |
| `canopee-cli` | `canopee`: the command-line tool built on `canopee-sdk` |
| `canopee-gateway` | Loopback WebSocket bridge: browser JS ⇄ node |
| `canopee-e2e` | End-to-end tests spawning real node processes against the real stack |

Each crate has its own `README.md` with implementation-level detail; these docs
describe how the pieces fit together.

## The key split: person identity vs device

- **Identity** — the *person*. One Ed25519 keypair per user, stored at
  `~/.canopee/identity/identity.key`, reused across all of that person's
  devices. Its `IdentityId` is `canopee://identity/<peer-id-from-identity-key>`.
- **DeviceKey** — this *machine*. One Ed25519 keypair per device, stored at
  `~/.canopee/identity/device.key`, never shared. The device key's public key
  is the machine's network `PeerId`.

Separation is why two devices of one identity can be online at once without
colliding: each announces a distinct `PeerId`, and the `(owner, "devices")`
record + `device:<peer-id>` registry map between the two worlds
(see [Identity](identity.md)).

## One runtime, three scopes

`Runtime` can open in three ways ([`canopee-runtime`]):

- `open()` — the standalone node: `~/.canopee` is the single root.
- `open_with_root(dir)` — fully isolated state: identity, storage, records,
  everything in `dir`. Used for embedded apps that want their own identity.
- `open_with_user_root(app_root)` — **"state is per-app, data is per-user"**:
  the identity, the user object store, and the record cache live in the shared
  `~/.canopee`, while network state, cache, socket, and exports are rooted in
  `app_root`. Several apps on one machine share one identity and one set of
  objects without colliding on the network. mDNS is off by default in this
  mode.

## What the runtime does on open

`Runtime::open_with_config` (in `crates/canopee-runtime/src/lib.rs`) is the
orchestration:

1. Load or create the person identity (+ optional at-rest encryption via
   `CANOPEE_IDENTITY_PASS`).
2. Load the per-device `DeviceKey`.
3. Open the object `Storage` and the LRU `Cache`.
4. Build the `NetworkManager` swarm (mDNS toggle from config).
5. **Reseed network serving**: re-share `shared: true` home-index entries.
6. Re-announce the public user records (profile, username, devices).
7. Register this device in `(owner, "devices")` + the device registry.
8. Spawn the LAN pairing handler and the periodic record-sync task.

The runtime's internals are split across domain modules —
`records.rs` (pointers, profile, devices, contacts), `sharing.rs` (home index,
announce/fetch), `capabilities.rs`, `pairing.rs`, `sync.rs` — each an `impl
Runtime` block over the single `struct Runtime`.

## The data path for one `put`

1. `CanopeeClient::put("hello")` → `NodeCommand` over the Unix socket.
2. `canopee-node` forwards to its embedded `Runtime`.
3. `Object::new(&identity, data, Blob)` signs the bytes and hashes them.
4. `Storage::put_verified` writes the object to `~/.canopee/storage/<id>`.
5. The `ObjectId` is returned to the caller; the object itself is **not**
   shared on the network until an explicit share action ([Sharing](sharing.md)).