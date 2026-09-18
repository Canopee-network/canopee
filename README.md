# Canopee

A decentralized peer-to-peer network of nodes. Each node owns a cryptographic
identity, stores signed, content-addressed objects locally, and can discover,
connect to, and exchange data with other nodes over libp2p — on the LAN via
mDNS, or across the internet via the DHT, relays, and hole punching.

The goal is a foundation apps can build on: any app talks to the local node
(through `canopee-sdk`, or by embedding `canopee-runtime` directly) to store
data, discover and fetch objects from peers, and publish/subscribe to topics
for real-time communication — without running its own network stack. **Nothing
is shared by default**; every object is signed and content-verifiable by
anyone.

## Documentation

- **New here?** Start with [`docs/README.md`](docs/README.md), or jump
  straight to the [Quickstart](docs/guides/quickstart.md) to get a node up in
  about five minutes.
- **Understand the model:** [Concepts](docs/concepts/README.md) — identity,
  objects & pointers, networking, sharing, capabilities, security.
- **Do something hands-on:** [Guides](docs/guides/README.md) — sharing,
  chat, multi-device, publishing apps, Tauri tutorials, and more.
- **Check the exacts:** [Reference](docs/reference/README.md) — every CLI
  command, environment variable, filesystem path, protocol, and SDK method.
- **For contributors:** [`ONBOARDING.md`](ONBOARDING.md) walks through
  building, testing, and working in this repo.

## Quick start

```bash
# Build everything
cargo build --workspace

# Initialize, start, and use a node
canopee init
canopee start
canopee identity      # canopee://identity/<peer-id>
canopee put ./file.txt
canopee list
canopee stop
```

Or build an app against it directly:

```rust
use canopee_sdk::CanopeeClient;

let client = CanopeeClient::connect().await?;
let id = client.put(b"hello canopee".to_vec()).await?;
println!("stored as {id}");
```

## What it is

- **One identity per person, one device key per machine.** A person's
  Ed25519 account key signs everything they publish; each machine holds a
  separate per-device key whose public key is the network `PeerId`. The same
  identity can live on several devices at once ([Identity](docs/concepts/identity.md)).
- **Objects and records.** Content-addressed, signed, immutable objects;
  mutable pointers resolve `(owner, name)` to the latest object. Your profile,
  contacts, home index, devices, username, and capabilities are all just
  records ([Objects & pointers](docs/concepts/objects.md)).
- **Nothing shared by default.** `put` stores locally; `share` is the explicit
  "serve this to the network" act ([Sharing](docs/concepts/sharing.md)).
  Capabilities layer signed, verifiable grants on top for who-may-do-what
  ([Capabilities](docs/concepts/capabilities.md)).
- **Static apps as first-class objects.** Publish a directory as a signed
  manifest, announce it on the DHT, and any peer can open it in a browser
  by name ([Publishing apps](docs/guides/publishing-apps.md)).
- **Serving is a local, loopback affair.** The node speaks its protocol over
  a Unix socket; browser access goes through a loopback WebSocket gateway
  with a session token ([Gateway](docs/guides/gateway.md)).

## Workspace layout

Canopee is a Cargo workspace. Each crate has its own README with full
details; this is the map of how they fit together.

```
                          ┌────────────────┐
                          │  canopee-cli   │  binary: `canopee`
                          │  canopee-node  │  binary: `canopee-node`
                          └───────┬────────┘
                                  │ owns / drives
                          ┌───────▼─────────┐
                          │ canopee-runtime │  ties identity + storage + network together
                          └───┬───────┬─────┘
                 ┌────────────┘       └───────────┐
        ┌────────▼────────┐               ┌───────▼─────────┐
        │ canopee-storage │               │ canopee-network │  libp2p swarm
        └────────┬────────┘               └───────┬─────────┘
                  │                               │
          ┌───────▼──────────┐                    │
          │ canopee-identity │◄───────────────────┘
          └──────────────────┘

     canopee-protocol   wire format between the node and its clients
     canopee-config     shared filesystem paths (~/.canopee/...) + env vars
     canopee-sdk        client library for apps — talks to the node over its
                        Unix socket using canopee-protocol
     canopee-gateway    local WebSocket bridge so a browser tab can drive
                        the node through canopee-sdk
     canopee-e2e        end-to-end tests spawning real node processes
```

| Crate | What it is |
|---|---|
| `canopee-identity` | Ed25519 account keypairs, X25519 DH keys, signing, verification, device keys, pairing crypto |
| `canopee-storage` | Content-addressed, signed `Object` store on disk; user records; capabilities; DHT record model |
| `canopee-network` | libp2p swarm: discovery (mDNS, Kademlia), DHT provider/record store, object exchange, pairing, gossipsub, relay/DCUtR/autonat |
| `canopee-config` | `Config`: the `~/.canopee` layout (identity, storage, records, cache, exports, socket) + env-var overrides |
| `canopee-runtime` | `Runtime`: ties identity + storage + network into one node runtime; implements every high-level operation |
| `canopee-protocol` | The wire types (`NodeCommand` / `NodeResponse`, `PairingData`, `SyncResult`, …) shared between node and client |
| `canopee-node` | The daemon: serves `canopee-protocol` over a Unix socket; also embeds a `Runtime` |
| `canopee-sdk` | `CanopeeClient`: the client library apps use |
| `canopee-cli` | `canopee`: the command-line tool built on `canopee-sdk` |
| `canopee-gateway` | Loopback WebSocket bridge: browser JS ⇄ node |
| `canopee-e2e` | End-to-end tests spawning real node processes against the real stack |

## Node state

Everything a node owns lives under `~/.canopee/` (see
[Filesystem layout](docs/reference/filesystem.md)):

```
~/.canopee/
├── identity/          # account key (the person) + device key (this machine)
├── storage/           # signed content-addressed objects
├── records/           # local cache of resolved (owner, name) pointers
├── aliases/           # friendly-name → owner map for canopee:// URIs
├── state/             # node runtime state
├── cache.cache        # LRU sidecar (evicts fetched content, never your own)
├── exports/           # *.canopee export bundles
├── node.sock          # the Unix socket the daemon speaks on
└── uri-pending        # canopee:// URIs handled while no node was running
```

## Status

This is an active work-in-progress reference implementation, not a hardened
production system. What's implemented, what's a known limitation, and what's
deliberately left to the app layer are spelled out in
[Security concept](docs/concepts/security.md) and [Roadmap](docs/reference/roadmap.md).