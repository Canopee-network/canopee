# Onboarding

Welcome to Canopee. This guide takes you from zero to productive: what the
project is, how the repository is laid out, how to build and test, and the
concepts you'll touch day to day.

If you haven't already, read the [README](README.md) first — it covers the
vision and the crate map. This file is the practical "how do I work here"
companion.

---

## What Canopee is

Canopee is a decentralized peer-to-peer network of nodes. Each node owns a
cryptographic identity, stores signed content-addressed objects locally, and
discovers/connects to other nodes over libp2p (mDNS on LAN, Kademlia DHT +
relays over the internet). Apps talk to the local node over a Unix socket via
`canopee-sdk`, or embed `canopee-runtime` directly — they never touch the
network stack by hand.

The current focus is **multi-device identity**: one identity shared across a
laptop, a phone, a desktop — via LAN pairing (QR + typed code), DHT-based
sync, and per-device keys.

---

## Repository layout

The workspace is `canopee/`. Sibling directories hold apps built on it.

```
CANOPEE/
├── canopee/                    # ← you are here (the workspace)
│   ├── crates/
│   │   ├── canopee-e2e         # end-to-end tests (real node processes)
│   │   ├── canopee-cli         # `canopee` command-line tool (binary)
│   │   ├── canopee-config      # ~/.canopee filesystem layout + env vars
│   │   ├── canopee-gateway     # loopback WebSocket bridge (browser → node)
│   │   ├── canopee-identity    # Ed25519 keypairs, signing, pairing crypto
│   │   ├── canopee-network     # libp2p swarm (mDNS, DHT, gossipsub, relay)
│   │   ├── canopee-node        # the node daemon (Unix socket server)
│   │   ├── canopee-protocol    # wire types (node ↔ SDK/CLI)
│   │   ├── canopee-runtime     # ties identity + storage + network together
│   │   ├── canopee-sdk         # client library for apps
│   │   └── canopee-storage     # signed content-addressed object store
│   ├── deploy/                 # systemd service setup for a public relay
│   ├── docs/                   # → docs/README.md: the documentation tree
│   │   ├── concepts/           #   how it works
│   │   ├── guides/             #   how to do it
│   │   └── reference/          #   the exacts (CLI, env, paths, protocols)
│   ├── scripts/e2e.sh          # two-node end-to-end shell test
│   ├── ONBOARDING.md           # ← you are here
│   └── README.md               # overview / crate map / docs index
```

Read [docs/README.md](docs/README.md) for the full documentation index — the
concepts/guides/reference split is the intended entry points.

---

## Building

Requires a Rust toolchain (edition 2024 — a recent stable).

```bash
cargo build --workspace
```

Everything compiles as one Cargo workspace (resolver 3, 11 member crates,
all listed in the root `Cargo.toml`).

## Testing

Three tiers, cheapest to most realistic **(E2E must run single-threaded —
see the note below)**:

```bash
# 1. Unit tests
cargo test --workspace

# 2. Real-swarm integration (networking: dial + mDNS + request/response)
cargo test -p canopee-network

# 3. End-to-end: real nodes + real CLI + real DHT, isolated via CANOPEE_APP_ROOT
cargo test -p canopee-e2e -- --test-threads=1
```

The full pre-release gate is:

```bash
cargo build --workspace
cargo test --workspace -- --test-threads=1
```

**Why `--test-threads=1` for e2e:** the e2e tests bind real ports/sockets and
spawn real processes; parallel runners can collide. This is documented in the
tests and enforced by running them serially.

You can also run the same scenario as a plain shell script (good for CI
without the cargo harness):

```bash
scripts/e2e.sh
```

What e2e actually proves (from `crates/canopee-e2e`): two nodes discover each
other via mDNS; an object is private by default (fetch refused); after
`share` it is discoverable as a provider and fetchable; after `unshare` the
fetch is refused again.

---

## Key concepts (5-minute tour)

The full detail is in [docs/concepts](docs/concepts/README.md); here's the
shape of it:

1. **Identity = the person, device key = the machine.** A person's Ed25519
   account key (`identity.key`) signs everything they publish and is reused
   across devices. Each machine holds a distinct per-device key
   (`device.key`) whose public key is its libp2p `PeerId` — so the same
   identity can be online on several devices at once.
2. **Objects are immutable and content-addressed; pointers are the mutable
   layer.** `id = hash(type ‖ data)`. Records (`(owner, name) → object`)
   — profile, contacts, home index, devices, username, capabilities — are
   the "what's the latest for this name" lookups.
3. **Nothing is shared by default.** `put` stores locally. `share` is the
   explicit act: upsert a shared home entry, publish a pointer, announce a
   DHT provider. `unshare` withdraws.
4. **Records and registries live on the DHT**, signed so the swarm can't
   forge them: provider records, `username:<name>`, `device:<peer-id>`,
   and `(owner, name)` pointers.
5. **Capabilities are signed grants** — `issuer → subject, permissions over
   a resource` — verifiable offline, tracked in the `(owner, "capabilities")`
   record, enforced by the runtime before serving a resource.
6. **Serving is local.** The node speaks `canopee-protocol` on a Unix socket.
   Browser access goes through the loopback-only, token-gated
   `canopee gateway`.

## Working conventions

- Each crate has its own `README.md` with implementation-level detail; the
  crate map and docs index link to all of them.
- The runtime's internals are split across domain modules — `records.rs`,
  `sharing.rs`, `capabilities.rs`, `pairing.rs`, `sync.rs` — each an `impl
  Runtime` block over the single `struct Runtime` in `lib.rs`. Keep new
  runtime operations in the module they belong to, and mark cross-module
  private items `pub(crate)`.
- The network manager is split between `src/manager.rs` and its
  `src/manager/event_loop.rs` submodule (the swarm event loop). Free
  functions there are `pub(super)`.
- Wire message types used by both the node and clients live in
  `canopee-protocol`; storage/user-data types live in `canopee-storage`;
  the SDK (`canopee-sdk`) wraps node commands as async client methods.
- CLI commands are per-domain files under `crates/canopee-cli/src/commands/`
  (`identity.rs`, `sharing.rs`, `capabilities.rs`, `pairing.rs`, …).

## Where to go next

- [docs/guides/quickstart.md](docs/guides/quickstart.md) — get a node running
  and your first objects stored.
- [docs/guides/testing.md](docs/guides/testing.md) — the full testing
  picture.
- [docs/concepts/architecture.md](docs/concepts/architecture.md) — the crate
  map and data flow in depth.
- [docs/reference/cli.md](docs/reference/cli.md) — every `canopee` command.