# canopee-network

The libp2p-backed peer-to-peer layer for a Canopee node: peer discovery (LAN
and DHT), direct object exchange, pub/sub messaging, and NAT traversal. This
crate owns the entire `libp2p::Swarm` and exposes it as an async command/event
API (`NetworkManager`) so the rest of the codebase never touches libp2p
types directly.

## Behaviours

`CanopeeBehaviour` combines ten libp2p protocols into one swarm:

| Behaviour | Role |
|---|---|
| `identify` | Exchanges protocol version + listen addresses with each peer on connect |
| `kad` (Kademlia) | DHT for wide-area peer discovery and object provider records; runs in `Server` mode so every node participates in routing, not just queries |
| `mdns` | Automatic discovery of peers on the local network (multicast DNS); auto-dials whatever it finds |
| `object_exchange` (request-response, CBOR) | Direct object transfer: `ObjectRequest::GetObject(id)` → `ObjectResponse::Object(bundle)` \| `NotFound` |
| `ping` | Liveness/RTT for established connections |
| `gossipsub` | Topic-based pub/sub messaging, signed by each publisher's identity |
| `relay` (server) | Every node can relay traffic for a peer it can't otherwise reach — there's no dedicated relay infrastructure |
| `relay_client` | Lets this node request a circuit reservation through another node acting as relay |
| `dcutr` | Once relayed, attempts to upgrade the connection to a direct one (hole punching) |
| `autonat` | Figures out whether this node is publicly reachable or behind a NAT |

## `NetworkManager`

`NetworkManager::new` builds and starts the swarm, then spawns a single task
that owns it — the `Swarm` isn't `Send`-shareable across tasks, so every
operation goes through an `mpsc` command channel with `oneshot` (or
`broadcast`, for pub/sub) replies. `NetworkManager` itself is just a cheap,
`Clone`-able handle to that channel.

```rust
use canopee_network::{NetworkManager, ObjectProvider};
use std::sync::Arc;

let network = NetworkManager::new(identity, "/ip4/0.0.0.0/tcp/0".parse()?, object_provider)?;
```

`object_provider: Arc<dyn ObjectProvider>` is how this crate stays decoupled
from [`canopee-storage`](../canopee-storage): it's asked to resolve an
`ObjectId` to an `ExportBundle` whenever a peer requests one over
`object_exchange`, without depending on the concrete `Storage` type.
[`canopee-runtime`](../canopee-runtime) provides the real implementation.

### Discovery & objects

```rust
network.dial(addr).await?;                      // connect directly to a known multiaddr
let peers = network.peers().await?;              // currently connected peers

network.announce(object_id).await?;               // "I have this object" (DHT provider record)
let providers = network.find_providers(object_id).await?; // who else has it
let bundle = network.get_object(peer_id, object_id).await?; // fetch it directly from them
```

Peer discovery is layered: `mdns` finds peers on the LAN and dials them
automatically; every successful connection and every `identify` exchange
feeds addresses into Kademlia's routing table, so wide-area lookups
(`find_providers`) work as soon as the swarm has any path into the DHT.

### Pub/sub

```rust
network.publish("app-topic", b"hello".to_vec()).await?; // fails with InsufficientPeers if no one's subscribed
let mut rx = network.subscribe("app-topic").await?;      // broadcast::Receiver<PubSubMessage>
while let Ok(msg) = rx.recv().await {
    if msg.topic == "app-topic" { /* ... */ }
}
network.unsubscribe("app-topic").await?;
```

`subscribe` returns a fresh `broadcast::Receiver` bound to a single internal
channel shared across all subscribed topics — filter on `PubSubMessage::topic`
if you subscribe to more than one topic per manager.

### Hole punching

```rust
network.listen_via_relay(relay_multiaddr).await?; // relay_multiaddr must include /p2p/<relay peer id>
```

This requests a circuit reservation on a specific relay (any reachable
Canopee node works as one — see the `relay` row above) so peers behind other
NATs can open a connection to this node via `<relay_addr>/p2p-circuit`. Once
that relayed connection exists, `dcutr` automatically attempts to upgrade it
to a direct connection in the background; nothing further needs to be called.

## Design notes

- **Every node is a relay.** There's no bootstrap/relay-server tier — any
  publicly reachable Canopee node can relay for a NAT'd one, matching the
  fully decentralized model. The tradeoff is every node pays a small
  relay-serving cost even if it never needs relaying itself.
- **The swarm lives in one task.** All mutation goes through
  `handle_command`/`handle_swarm_event` in `manager.rs`'s event loop — there
  is no locking because there's no shared mutable state; everything is
  message-passed.
- **`kad::Mode::Server` is required**, not just the default client mode —
  without it, a node won't answer other peers' DHT queries or store
  provider records, which silently breaks discovery between two peers that
  are both only "clients." This was a real bug caught during development:
  see the `two_nodes_dial_and_discover_via_kad` test.
- **mDNS discovery isn't guaranteed to work everywhere** — some sandboxes
  and CI environments block multicast. Explicit `dial()` (or DHT-based
  discovery once any peer is reachable) is the fallback path and is what
  the test suite and CLI actually exercise.
- **Custom protocol strings** (`/canopee/id/1.0.0`, `/canopee/kad/1.0.0`,
  `/canopee/objects/1.0.0`) namespace this swarm from other libp2p networks
  — two Canopee nodes will only speak Kademlia/identify/object-exchange to
  each other, not to unrelated libp2p software using the same default
  protocol strings.

## Testing

```bash
cargo test -p canopee-network
```

The integration tests in `tests/handshake.rs` spin up two or three real
swarms on localhost and drive them through `NetworkManager`, covering:
peer discovery via direct dial, DHT provider announce → find → fetch, and
gossipsub publish → subscribe delivery. They run best with
`--test-threads=1` to avoid port contention across parallel tests.
