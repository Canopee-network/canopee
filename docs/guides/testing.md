# Running the test suite

Canopee tests at four levels. From cheapest to most realistic:

## 1. Unit tests (cargo test)

```bash
cargo test --workspace
```

Unit tests for each crate: `canopee-identity` (keys, signing, pairing
crypto), `canopee-storage` (objects, pointers, capabilities, cache,
export/import), `canopee-network` (swarm command handling), `canopee-runtime`
(objects/records/sharing/capabilities/sync), `canopee-config` (paths),
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

`crates/canopee-e2e/tests/e2e.rs` (and `scripts/e2e.sh`, the identical shell
version) spawns two real `canopee-node` processes in isolated homes and
drives them through the real `canopee-cli`, over the real DHT + libp2p:

1. Nodes A and B start and discover each other via mDNS (localhost).
2. A stores a file — **private by default**: B's fetch is refused.
3. A shares it; B discovers A as a DHT provider and fetches the object.
4. A unshares it; B's fetch is refused again.

**Important: E2E tests must run single-threaded** (`--test-threads=1`),
because they bind real ports/sockets and spawn real processes — parallel
runners can interfere. This is documented in the tests themselves.

## 4. The shell e2e

```bash
scripts/e2e.sh
```

The same scenario, as a self-contained bash script with automatic cleanup —
useful when you want the exit-code check without cargo's test harness.

## Full gate

The complete pre-release check is:

```bash
cargo build --workspace
cargo test --workspace -- --test-threads=1
```

Build first, then run everything single-threaded so the socket/port-bound
tests serialize. A passing gate means: all crates compile, all unit tests
pass, both swarms talk over the real network, and the object
share → discover → fetch → unshare flow works across two real process trees.