# Canopee

Canopee is a decentralized peer-to-peer network of nodes. Each node owns a
cryptographic identity, stores signed content-addressed objects locally, and
can discover, connect to, and exchange data with other nodes over libp2p —
directly, over the internet via relays and hole punching, or on the local
network via mDNS.

The goal is a foundation apps can build on: any app can talk to the local
node (through [`canopee-sdk`](crates/canopee-sdk)) to store data, discover
and fetch objects from peers, and publish/subscribe to topics for real-time
communication — without needing to run their own network stack.

New to P2P networking or NAT/relays/DHTs? Start with
[`docs/networking-for-beginners.md`](docs/networking-for-beginners.md) — a
no-prior-knowledge walkthrough of the concepts and how to set up and connect
nodes. Want to try it hands-on right away? See
[`docs/testing-chat-between-peers.md`](docs/testing-chat-between-peers.md) for
a step-by-step walkthrough of chatting between two peers, on the same LAN or
through a relay. Want to publish and open a small static app (e.g. a
portfolio site) peer-to-peer? See
[`docs/app-manifests.md`](docs/app-manifests.md) for publishing, announcing,
fetching, and serving app manifests. Want to go further and make published
apps survive the original publisher going offline? See
[`docs/p2p-app-caching-tutorial.md`](docs/p2p-app-caching-tutorial.md) — a
step-by-step, learn-by-doing guide (no code given) to implementing
fetch-and-reannounce caching yourself.

## Workspace layout

Canopee is a Cargo workspace. Each crate has its own README with full details;
this is the map of how they fit together.

```
                          ┌────────────────┐
                          │  canopee-cli   │  binary: `canopee`
                          │  canopee-node  │  binary: `canopee-node`
                          └───────┬────────┘
                                  │ owns
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

     canopee-protocol   wire format between node and clients (SDK/CLI)
     canopee-config     shared filesystem paths (~/.canopee/...)
     canopee-sdk        client library for apps — talks to the node over
                         its Unix socket using canopee-protocol
```

| Crate | What it is | README |
|---|---|---|
| [`canopee-identity`](crates/canopee-identity) | Ed25519 keypairs, signing, verification | [README](crates/canopee-identity/README.md) |
| [`canopee-storage`](crates/canopee-storage) | Content-addressed, signed object store on disk | [README](crates/canopee-storage/README.md) |
| [`canopee-network`](crates/canopee-network) | libp2p swarm: discovery, DHT, pub/sub, hole punching | [README](crates/canopee-network/README.md) |
| [`canopee-config`](crates/canopee-config) | Shared `~/.canopee` filesystem layout | [README](crates/canopee-config/README.md) |
| [`canopee-runtime`](crates/canopee-runtime) | Wires identity + storage + network into one node runtime | [README](crates/canopee-runtime/README.md) |
| [`canopee-protocol`](crates/canopee-protocol) | Wire types shared between the node and its clients | [README](crates/canopee-protocol/README.md) |
| [`canopee-node`](crates/canopee-node) | The node daemon — binary + library, serves the Unix socket | [README](crates/canopee-node/README.md) |
| [`canopee-sdk`](crates/canopee-sdk) | Client library for apps: identity, storage, network | [README](crates/canopee-sdk/README.md) |
| [`canopee-cli`](crates/canopee-cli) | `canopee` command-line tool | [README](crates/canopee-cli/README.md) |

## Quick start

```bash
# Build everything
cargo build --workspace

# Start a node in the background
cargo run -p canopee-node &

# Talk to it with the CLI
cargo run -p canopee-cli -- identity
cargo run -p canopee-cli -- put ./some-file.txt
cargo run -p canopee-cli -- list
cargo run -p canopee-cli -- status
cargo run -p canopee-cli -- stop
```

Or build an app against it directly with [`canopee-sdk`](crates/canopee-sdk):

```rust
use canopee_sdk::CanopeeClient;

let client = CanopeeClient::connect().await?;
let id = client.put(b"hello canopee".to_vec()).await?;
println!("stored as {id}");
```

## Node state

Everything a node owns lives under `~/.canopee/` (see
[`canopee-config`](crates/canopee-config/README.md)):

```
~/.canopee/
├── identity/identity.key   # Ed25519 keypair (create once, reused forever)
├── storage/                # signed objects, one file per object id
├── exports/                # *.canopee export bundles
├── state/node.state        # small metadata: identity, started flag, timestamps
└── node.sock                # Unix socket the node listens on
```

## Status

This is an active work-in-progress reference implementation, not a hardened
production system. See each crate's README for what's implemented and what's
still a placeholder or a known limitation.
