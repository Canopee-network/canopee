# Running the test suite

Canopee tests at four levels. From cheapest to most realistic:

## 1. Unit tests (cargo test)

```bash
cargo test --workspace --exclude canopee-e2e
```

Unit tests for each crate: `canopee-identity` (keys, signing, pairing
crypto), `canopee-storage` (objects, pointers, capabilities, cache,
export/import), `canopee-network` (swarm command handling), `canopee-runtime`
(objects/records/sharing/capabilities/record sync/folder sync),
`canopee-config` (paths),
`canopee-cli` (command parsing), `canopee-gateway` (session/websocket).

Some use real-temp-directory storage or real swarm instances; they are
self-contained and fast.

## 2. Network integration (handshake)

```bash
cargo test -p canopee-network
```

`crates/canopee-network/tests/handshake.rs` spawns *real* swarm instances
and verifies dialing, mDNS discovery, and round-trip requests over the actual
libp2p stack — no mocks for the protocols that matter.

## 3. End-to-end (real processes, real CLI)

```bash
cargo test -p canopee-e2e -- --test-threads=1
```

`crates/canopee-e2e/tests/e2e.rs` spawns real `canopee-node` processes in
isolated homes and drives them through the real `canopee-cli`, over the real
DHT + libp2p. Scenarios include:

1. Share → discover → fetch → unshare, with private fetches refused.
2. Pairing two devices and restarting the new device under the owner identity.
3. Periodic user-record sync (profile refresh without a manual sync).
4. Folder sync: create/join, nested files, bidirectional edits, offline edits,
   deletes, conflict copies, delete-vs-edit, restart/tombstones, a multi-chunk
   file, and no plaintext file content or chunk probe in either object store.

**Important: E2E tests must run single-threaded** (`--test-threads=1`),
because they bind real ports/sockets and spawn real processes — parallel
runners can interfere. This is documented in the tests themselves.

## 4. The shell e2e

```bash
scripts/e2e.sh
```

The sharing scenario, as a self-contained bash script with automatic cleanup —
useful when you want the exit-code check without cargo's test harness.

## Full gate

The complete pre-release check is:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude canopee-e2e
cargo test -p canopee-e2e --test e2e -- --test-threads=1
```

The e2e step runs single-threaded so the socket/port-bound tests serialize. A
passing gate means: all crates compile cleanly, all unit tests pass, both
swarms talk over the real network, and the sharing, pairing, record-sync, and
folder-sync scenarios work across real process trees.
