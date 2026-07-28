# Testing chat between two peers

`canopee chat <topic>` is a small REPL over gossipsub: everyone subscribed to
the same topic sees everyone else's messages. It's the easiest way to
smoke-test that two nodes are actually connected and can exchange
pub/sub traffic. This doc walks through two setups:

- **Same LAN** — two nodes discover each other automatically via mDNS, or you
  simulate this with two node processes on one machine.
- **Via relay** — two nodes that can't reach each other directly (e.g. both
  behind home NAT, on different networks) connect through a relay.

If you're not familiar with mDNS/relays/circuit reservations yet, read
[`networking-for-beginners.md`](networking-for-beginners.md) first — this doc
assumes those concepts and focuses on the exact commands to run.

Each peer needs its own `~/.canopee` (identity + socket) — see
[Running two local nodes on one machine](#running-two-local-nodes-on-one-machine)
below if you're testing on a single laptop rather than two separate machines.

## Same LAN (mDNS)

If both machines are on the same Wi-Fi/LAN, no manual connection step is
needed — mDNS finds peers automatically.

**On peer A:**

```bash
cargo run -p canopee-node &
cargo run -p canopee-cli -- identity
# IdentityId("canopee://identity/12D3KooW...A")
```

**On peer B (separate machine or separate `$HOME`, see below):**

```bash
cargo run -p canopee-node &
cargo run -p canopee-cli -- identity
# IdentityId("canopee://identity/12D3KooW...B")
```

Give mDNS a couple of seconds, then confirm each side sees the other:

```bash
cargo run -p canopee-cli -- peers
# 12D3KooW...B
#   Address: /ip4/192.168.1.42/tcp/54321
```

If `peers` comes back empty after a few seconds, mDNS is likely blocked on
that network (common on corporate Wi-Fi and some CI/sandboxes) — skip to
[Via relay](#via-relay) or use `canopee dial <peer-B-multiaddr>` directly if
you know peer B's address.

Once both sides show at least one peer, chat:

**Peer A:**

```bash
cargo run -p canopee-cli -- chat general
Chatting on topic 'general'. Type a message and press enter to send.
hello from A
```

**Peer B:**

```bash
cargo run -p canopee-cli -- chat general
Chatting on topic 'general'. Type a message and press enter to send.
12D3KooW...A: hello from A
hello from B
```

If `chat`/`publish` fails with `InsufficientPeers`, gossipsub hasn't formed a
mesh for that topic yet — this usually means the peers aren't connected at
all yet (double-check `peers` on both sides) or one side hasn't subscribed to
the same topic string yet. Give it a second after both sides start `chat` and
retry.

## Via relay

Use this when the two peers can't reach each other directly — e.g. both are
behind home routers with no port forwarded, on different networks. You need
a third node with a publicly reachable address to act as the relay. See
[`deploy/README.md`](../deploy/README.md) for running one on a VPS long-term,
or spin up a throwaway one locally for a quick test (below).

### 1. Start (or point at) a relay

Any `canopee-node` can relay — it's built in and on by default. For a real
test across two networks you need one with a real public address (a cheap
VPS works); for a quick local dry run of the *mechanics* you can run a third
node on the same machine with a fixed port:

```bash
HOME=/tmp/canopee-relay CANOPEE_LISTEN_PORT=4001 cargo run -p canopee-node &
HOME=/tmp/canopee-relay cargo run -p canopee-cli -- identity
# IdentityId("canopee://identity/12D3KooW...Relay")
```

(`canopee-config` resolves `~/.canopee` from the process's real `$HOME` — see
[Running two local nodes on one machine](#running-two-local-nodes-on-one-machine)
for why overriding it is how you get independent identities on one machine.)

(A fully local dry run still won't need the relay — peer A and peer B on the
same machine can already reach each other directly. It's useful for
confirming the reservation/circuit-address mechanics below before you deploy
a real internet-facing relay.)

Note its multiaddr: `/ip4/<relay-ip>/tcp/4001/p2p/12D3KooW...Relay` — use
`127.0.0.1` for the local dry run, the VPS's public IP for a real test.

### 2. Both peers request a circuit reservation

**Peer A:**

```bash
cargo run -p canopee-cli -- listen-via-relay /ip4/<relay-ip>/tcp/4001/p2p/12D3KooW...Relay
Requesting relay reservation...
```

**Peer B:** same command, same relay address.

### 3. Confirm the reservation actually succeeded

`listen-via-relay` only *requests* the reservation — it returns immediately
and doesn't tell you whether the relay accepted it. Check with:

```bash
cargo run -p canopee-cli -- relay-status
Relay: 12D3KooW...Relay
  Renewal: false
  Listen address: /ip4/<relay-ip>/tcp/4001/p2p/12D3KooW...Relay/p2p-circuit/p2p/12D3KooW...A
```

Run this on both peers before moving on. If it prints "No accepted relay
reservations yet", wait a moment and retry — if it still doesn't show up,
double-check the relay is actually running and reachable at that address
(`canopee dial <relay-address>` from each peer should succeed first).

The `Listen address` line is the exact multiaddr other peers can now use to
reach you — copy it down.

### 4. Dial each other through the relay

**Peer A dials peer B's circuit address** (from B's `relay-status` output):

```bash
cargo run -p canopee-cli -- dial /ip4/<relay-ip>/tcp/4001/p2p/12D3KooW...Relay/p2p-circuit/p2p/12D3KooW...B
```

Confirm the connection landed on both sides:

```bash
cargo run -p canopee-cli -- peers
# 12D3KooW...B
```

Once connected through the relay, both sides automatically attempt **hole
punching** (dcutr) in the background to upgrade to a direct connection —
nothing to do here, it either succeeds silently or traffic keeps flowing
through the relay.

### 5. Chat

Same as the LAN case, once `peers` shows the connection:

**Peer A:**

```bash
cargo run -p canopee-cli -- chat general
```

**Peer B:**

```bash
cargo run -p canopee-cli -- chat general
```

## Running two local nodes on one machine

`~/.canopee` (identity, storage, and the `node.sock` the CLI talks to) is
shared per-`$HOME`, so two node processes with the same `$HOME` collide — the
second `canopee-node` will fail to bind or will overwrite the first's
socket. To run two independent "peers" on one machine for testing, give each
its own home directory and run every command for that peer with the matching
`CANOPEE_HOME`:

```bash
# Peer A
HOME=/tmp/canopee-a cargo run -p canopee-node &
HOME=/tmp/canopee-a cargo run -p canopee-cli -- identity

# Peer B
HOME=/tmp/canopee-b cargo run -p canopee-node &
HOME=/tmp/canopee-b cargo run -p canopee-cli -- identity
```

`canopee-config` resolves `~/.canopee` via `dirs::home_dir()` (the process's
real `$HOME`) — there's no separate `CANOPEE_HOME` override, so overriding
`$HOME` itself is the only way to get two independent identities/sockets on
one machine today. Every command for a given peer (`identity`, `status`,
`chat`, etc.) needs the same `HOME=...` prefix, or you'll hit the other
peer's socket instead.

Two nodes on one machine's loopback interface will find each other via mDNS
or direct dial just like two machines on a LAN — this is only useful for
exercising the CLI flow and the relay reservation mechanics, not for a
realistic NAT-traversal test (loopback has no NAT to punch through).

## Troubleshooting checklist

| Symptom | Check |
|---|---|
| `peers` is empty on both sides | Are both nodes actually running (`canopee status`)? Same LAN and mDNS not blocked? Try `canopee dial <addr>` directly. |
| `relay-status` shows no reservations | Is the relay reachable (`canopee dial <relay-addr>` succeeds)? Give it a few seconds after `listen-via-relay`. |
| `chat`/`publish` → `Error: Failed to publish: InsufficientPeers` | `peers` doesn't show the other side yet — connect first, gossipsub needs an established connection before it can mesh on a topic. |
| Messages don't arrive after connecting | Are both sides using the exact same topic string (`chat general` vs `chat General` are different topics)? |

## Where to go next

- [`networking-for-beginners.md`](networking-for-beginners.md) — the concepts
  behind mDNS, the DHT, relays, and hole punching
- [`deploy/README.md`](../deploy/README.md) — running a long-lived relay on a
  VPS instead of a throwaway local one
- [`canopee-cli` README](../crates/canopee-cli/README.md) — every CLI command
