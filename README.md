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
nodes. Wondering how this compares to IPFS, Iroh, or other P2P projects?
See [`docs/comparison.md`](docs/comparison.md).

**Tutorials**, roughly in the order you'd want them:

- [`docs/testing-chat-between-peers.md`](docs/testing-chat-between-peers.md) —
  hands-on walkthrough of chatting between two peers, on the same LAN or
  through a relay.
- [`docs/app-manifests.md`](docs/app-manifests.md) — publishing, announcing,
  resolving, fetching, and serving a static app/site peer-to-peer.
- [`docs/canopee-uri-scheme.md`](docs/canopee-uri-scheme.md) — typing
  `canopee://alice/portfolio` into a normal browser via the OS scheme handler.
- [`docs/publishing-vs-building-apps.md`](docs/publishing-vs-building-apps.md) —
  the difference between publishing static content via `app-manifest` and
  building a real program on [`canopee-sdk`](crates/canopee-sdk); read this
  if you're not sure which one you want.
- [`docs/tauri-presence-app-tutorial.md`](docs/tauri-presence-app-tutorial.md) —
  the smallest real `canopee-sdk` app: a desktop "who's online" presence
  indicator. Good starting point before the two below.
- [`docs/tauri-chat-app-tutorial.md`](docs/tauri-chat-app-tutorial.md) — a
  learn-by-doing guide to building a desktop chat app on `canopee-sdk`/
  `canopee-runtime` with an embedded node, so installing the app is the
  only setup step.
- [`docs/tauri-multiplayer-game-tutorial.md`](docs/tauri-multiplayer-game-tutorial.md) —
  a learn-by-doing guide to a desktop multiplayer game on the same
  embedded-node foundation, covering lockstep move ordering and state
  divergence detection on top of gossipsub's unordered, best-effort
  delivery. Builds directly on the chat tutorial above.
- [`docs/tauri-collab-editor-tutorial.md`](docs/tauri-collab-editor-tutorial.md) —
  a learn-by-doing guide to a peer-to-peer collaborative document editor,
  using a CRDT to merge concurrent edits automatically rather than
  detecting and rejecting conflicts.
- [`docs/spa-hosting-tutorial.md`](docs/spa-hosting-tutorial.md) — a
  learn-by-doing guide to publishing a real React/Vite build (client-side
  routing, MIME type coverage) rather than a hand-written page.
- [`docs/p2p-app-caching-tutorial.md`](docs/p2p-app-caching-tutorial.md) — a
  learn-by-doing guide to making published apps survive their original
  publisher going offline (fetch-and-reannounce caching).
- [`docs/bootstrap-nodes-tutorial.md`](docs/bootstrap-nodes-tutorial.md) — a
  learn-by-doing guide to community/public bootstrap relay lists, so a
  brand-new node can find its first peer without a human pasting a
  multiaddr.
- [`docs/multi-device-identity.md`](docs/multi-device-identity.md) — a
  learn-by-doing guide to using the same identity from a laptop and a
  phone: the honest manual key-copy version, an encrypted pairing-flow
  version, and what a real (cross-signed, independently revocable)
  multi-device design would need instead.

**Reference and roadmap docs** — not tutorials, but useful context:

- [`docs/app-ideas.md`](docs/app-ideas.md) — a longer list of possible
  apps beyond what has a tutorial yet, sorted by how well-scoped each one
  is.
- [`docs/roadmap-hosting-replacement.md`](docs/roadmap-hosting-replacement.md) —
  a gap analysis of what it would take for Canopee to genuinely replace a
  commercial hosting provider, and which gaps are buildable vs. which are
  structural tensions with being decentralized at all.
- [`docs/security-considerations.md`](docs/security-considerations.md) — a
  standing reference for what's handled (signed/content-addressed objects,
  verified app pointers) and what isn't yet (message/content encryption,
  key revocation, DHT/bootstrap trust, cache node accountability).
- [`docs/end-to-end-encryption.md`](docs/end-to-end-encryption.md) — what
  the X25519 key-agreement primitive in `canopee-identity` gives you, and
  the much larger set of things (encryption, ratcheting, group keys,
  key discovery) it deliberately doesn't — those remain app-layer work.

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
