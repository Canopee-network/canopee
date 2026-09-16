# Onboarding

Welcome to Canopee. This guide gets you from zero to productive: what the
project is, how the repo is laid out, how to build and test, and the key
concepts you'll touch day to day.

If you haven't already, read the [README](README.md) first — it covers the
project vision and the crate map. This file is the practical "how do I work
here" companion.

---

## What Canopee is

Canopee is a decentralized peer-to-peer network of nodes. Each node owns a
cryptographic identity, stores signed content-addressed objects locally, and
discovers/connects to other nodes over libp2p (mDNS on LAN, Kademlia DHT +
relays over the internet). Apps talk to the local node over a Unix socket via
`canopee-sdk` — they never touch the network stack directly.

The current focus is **multi-device identity**: one identity shared across a
laptop, a phone, a desktop — via LAN pairing (QR + typed code) and DHT-based
sync.

---

## Repository layout

The workspace is `canopee/`. Sibling directories hold apps built on it.

```
CANOPEE/
├── canopee/                    # ← you are here (the workspace)
│   ├── crates/
│   │   ├── canopee-identity    # Ed25519 keypairs, signing, pairing crypto
│   │   ├── canopee-storage     # signed content-addressed object store
│   │   ├── canopee-network     # libp2p swarm (mDNS, DHT, gossipsub, relay)
│   │   ├── canopee-config      # ~/.canopee filesystem layout + env vars
│   │   ├── canopee-runtime     # ties identity + storage + network together
│   │   ├── canopee-protocol    # wire types (node ↔ SDK/CLI)
│   │   ├── canopee-node        # the node daemon (Unix socket server)
│   │   ├── canopee-sdk         # client library for apps
│   │   ├── canopee-cli         # `canopee` command-line tool
│   │   ├── canopee-gateway     # WebSocket bridge (browser → node)
│   │   └── canopee-e2e         # end-to-end tests (real node processes)
│   └── docs/                   # tutorials + reference docs
├── apps/
│   └── canopee-home            # Tauri 2 chat/home app (GUI)
├── canopee-tray                # macOS tray app (embeds the node)
└── interest-discovery          # example SDK app (DHT-based peer discovery)
```

---

## Prerequisites

- **Rust** (stable, edition 2024). `rustup` recommended.
- **Node.js ≥ 20.12** for the Tauri frontend (Vite 8). The default `node` on
  this machine is v16 — use `/opt/homebrew/bin/node` (v26) or `nvm use 20+`.
- **Xcode** (for iOS builds, optional).
- **Android SDK** (for Android builds, optional — not installed here).

---

## Quick start

```bash
# Build everything
cargo build --workspace

# Start a node in the background
cargo run -p canopee-node &

# Talk to it
cargo run -p canopee-cli -- identity
cargo run -p canopee-cli -- profile --name Alice
cargo run -p canopee-cli -- peers
cargo run -p canopee-cli -- stop
```

The node stores everything under `~/.canopee/` (identity, storage, records,
socket). See `canopee-config` for the layout.

---

## Key concepts

### Identity

- **`IdentityId`** (`canopee://identity/<peer-id>`) — the user's canonical
  identity, derived from an Ed25519 keypair. One per person.
- **`DeviceKey`** — a per-device keypair, distinct from the identity. Each
  device has its own `PeerId` (from the device key) but shares the identity.
  This is what lets two devices be "the same person" without sharing a
  network address.
- **Pairing** — copying an identity onto a new device over the LAN. The new
  device shows a 12-char code + QR payload; the existing device types the
  code to approve. Identity + signed records travel encrypted over
  `/canopee/pairing/1.0.0`.
- **Sync** — keeping profile/contacts/devices in step across paired devices.
  Last-writer-wins by the signed pointer's `published_at`. Runs manually
  (`canopee sync`) and periodically (30s background task).

### Storage

- **Objects** — signed, content-addressed blobs. The object id is the hash
  of its content. Immutable.
- **Pointers** (`AppPointerRecord`) — signed mappings from `(owner, name)` to
  an object id. Mutable (republishing overwrites). Stored on the DHT and in
  a local cache (`~/.canopee/records/`).
- **Records** — the three user records: `profile`, `contacts`, `devices`.
  Each is a pointer + the object it points to.

### Network

- **mDNS** — LAN discovery. Disable with `CANOPEE_MDNS=0` to simulate
  different networks.
- **Kademlia DHT** — internet-wide discovery + record storage. Bootstrap
  nodes seed the routing table.
- **gossipsub** — pub/sub for real-time messaging (chat rooms, presence).
- **Relay + DCUtR** — NAT traversal for nodes behind firewalls.

---

## Environment variables

These are read by `Config::new()` / `NetworkManager` and apply to the node,
SDK, CLI, and Tauri app.

| Variable | What it does | Default |
|---|---|---|
| `CANOPEE_APP_ROOT` | Replaces `~` as the base for `.canopee/` — full instance isolation (identity, storage, socket) | `~/.canopee` |
| `CANOPEE_MDNS` | `0`/`false`/`no`/`off` disables mDNS (simulate different networks) | on |
| `CANOPEE_BOOTSTRAP_ADDRS` | Comma-separated multiaddrs to dial at startup, replacing the defaults | public relay |
| `CANOPEE_BOOTSTRAP_ADDRS_PREPEND` | `1` prepends env addrs to defaults instead of replacing | off |
| `CANOPEE_IDENTITY_PASS` | Passphrase for at-rest identity encryption | none (plaintext) |
| `CANOPEE_DEVICE_NAME` | Human-friendly device name announced to peers | hostname |

---

## Testing

### Unit/integration tests (per crate)

```bash
cargo test -p canopee-identity
cargo test -p canopee-network
cargo test -p canopee-runtime
# ... etc
```

These run in-process and are fast. Run them before committing.

### End-to-end tests (real node processes)

```bash
# IMPORTANT: e2e tests must run serially
cargo test -p canopee-e2e --test e2e -- --test-threads=1
```

E2E tests spawn real `canopee-node` processes (separate `$HOME`s) and drive
them via the real `canopee` CLI over the real libp2p stack. They cover:
sharing/unsharing, identity export/import, LAN pairing, manual sync, and
periodic sync.

**Why serial?** Parallel execution lets nodes from different tests
cross-connect over mDNS/DHT and interfere nondeterministically. Serial is
the supported mode.

### Multi-instance testing (same machine)

Run multiple isolated nodes side-by-side with `CANOPEE_APP_ROOT`:

```bash
# Two isolated instances
CANOPEE_APP_ROOT=/tmp/alice ./target/debug/canopee-node &
CANOPEE_APP_ROOT=/tmp/bob   ./target/debug/canopee-node &

# Each CLI/GUI instance targets its own node
CANOPEE_APP_ROOT=/tmp/alice ./target/debug/canopee-cli identity
CANOPEE_APP_ROOT=/tmp/bob   ./target/debug/canopee-cli pair
```

### Simulating different networks

Turn off mDNS and use a private bootstrap node:

```bash
# Bootstrap node (isolated from public DHT)
CANOPEE_APP_ROOT=/tmp/bs CANOPEE_MDNS=0 CANOPEE_BOOTSTRAP_ADDRS= ./target/debug/canopee-node &

# Peers point at the private bootstrap
BS_ID=$(CANOPEE_APP_ROOT=/tmp/bs ./target/debug/canopee-cli device | head -n1)
PORT=$(lsof -nP -iTCP -sTCP:LISTEN | awk -v pid=$(pgrep -f canopee-node | head -1) '$2==pid{split($9,a,":");print a[2];exit}')
BS="/ip4/127.0.0.1/tcp/$PORT/p2p/$BS_ID"

CANOPEE_APP_ROOT=/tmp/a CANOPEE_MDNS=0 CANOPEE_BOOTSTRAP_ADDRS="$BS" ./target/debug/canopee-node &
CANOPEE_APP_ROOT=/tmp/b CANOPEE_MDNS=0 CANOPEE_BOOTSTRAP_ADDRS="$BS" ./target/debug/canopee-node &
```

Peers now discover each other purely over the DHT (no LAN shortcut) — the
same path two machines on different networks use.

---

## The Tauri app (`apps/canopee-home`)

The GUI chat/home app. Connects to a running node over the Unix socket.

```bash
# Start a node
CANOPEE_APP_ROOT=/tmp/alice ./target/debug/canopee-node &

# Run the app (needs Node ≥ 20.12)
CANOPEE_APP_ROOT=/tmp/alice PATH="/opt/homebrew/bin:$PATH" npm run tauri dev
```

Features: identity display, profile edit, QR pairing (show + scan), sync.
Camera scanning is mobile-only; desktop uses a paste-payload fallback.

---

## Multi-device identity roadmap

The current work is tracked in [`.opencode/plans/multi-device.md`](.opencode/plans/multi-device.md).
Status:

- **Phase 1** (device key separation) — done.
- **Phase 2** (identity export/import) — done.
- **Phase 3** (LAN pairing: QR + typed code) — done.
- **Phase 4** (sync protocol + periodic sync) — done.
- **Phase 5** (Tauri QR UI + mobile) — UI done; mobile verification pending
  (iOS simulator runtime download, Android SDK).

---

## Common tasks

### Add a CLI command

1. Add the variant to `Commands` in `crates/canopee-cli/src/main.rs`.
2. Add the match arm calling the SDK method.
3. If the SDK method doesn't exist, add it to `crates/canopee-sdk/src/client.rs`.
4. If the protocol command doesn't exist, add it to
   `crates/canopee-protocol/src/lib.rs` (`NodeCommand` + `NodeResponse`) and
   dispatch it in `crates/canopee-node/src/lib.rs`.

### Add a user record type

1. Define the struct in `crates/canopee-storage/src/user.rs` (with `version: u64`).
2. Add a `RECORD_*` constant.
3. Add `save_*`/`load_*` methods to `crates/canopee-runtime/src/lib.rs`.
4. Add it to the sync list in `sync_records()` if it should sync.

### Run the e2e suite before committing

```bash
cargo build --workspace
cargo test -p canopee-e2e --test e2e -- --test-threads=1
```

---

## Troubleshooting

| Problem | Fix |
|---|---|
| `vite build` fails with `styleText` | Node.js too old — use `/opt/homebrew/bin/node` (v26) or `nvm use 20+` |
| e2e tests fail in parallel | Run with `--test-threads=1` |
| Node can't find peers | Check `CANOPEE_MDNS` isn't `0`; check bootstrap addrs |
| `canopee-cli` can't connect | Is the node running? `canopee-cli start` or `cargo run -p canopee-node` |
| iOS build fails | Download the simulator runtime: `xcodebuild -downloadPlatform iOS` |
| `canopee pair` dial times out | Retry — a dial race was seen once; retry logic is a known follow-up |

---

## Where to look next

- [`docs/`](docs/) — tutorials (chat, apps, games, hosting, encryption).
- [`.opencode/plans/multi-device.md`](.opencode/plans/multi-device.md) — the
  multi-device identity spec + status.
- Each crate's `README.md` — implementation details per crate.
