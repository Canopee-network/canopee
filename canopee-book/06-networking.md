# Chapter 6: Networking

## The Network Layer

Canopee's networking is built on libp2p — the same peer-to-peer networking stack used by IPFS, Filecoin, and Polkadot. But Canopee uses a specific subset of libp2p's capabilities, chosen to minimize infrastructure requirements.

## The Swarm

At the center of the network layer is a single libp2p `Swarm` — the aggregate of all network behaviors, connections, and protocols. The swarm runs in its own tokio task; all operations go through an mpsc command channel.

```
Application → NetworkManager (Clone handle) → mpsc channel → Swarm task
```

`NetworkManager` is a cheap `Clone` handle. Multiple parts of the application can hold a clone and issue concurrent commands. The swarm processes them sequentially in its own task.

## Protocol Stack

Canopee registers ten behaviors on the swarm:

| Behavior | Protocol | Purpose |
|----------|----------|---------|
| Identify | `/canopee/id/1.0.0` | Exchange peer metadata |
| Kademlia | `/canopee/kad/1.0.0` | Distributed hash table |
| mDNS | — | Local network discovery |
| Object Exchange | `/canopee/objects/1.0.0` | CBOR request/response for objects |
| Ping | — | Latency measurement |
| Gossipsub | — | Pub/sub messaging |
| Relay (server) | — | Relay traffic for other peers |
| Relay Client | — | Use other peers as relays |
| DCUtR | — | Direct Connection Upgrade through Relay |
| AutoNAT | — | Detect NAT status |

The custom protocol strings (`/canopee/...`) namespace the swarm so Canopee nodes only speak to each other. You won't accidentally connect to an IPFS node.

## Discovery: How Peers Find Each Other

Peer discovery happens in layers:

### Layer 1: mDNS (Local Network)

On startup, the swarm broadcasts mDNS queries on the local network. Any Canopee node on the same LAN responds automatically. Connections are established without any configuration.

mDNS is the zero-configuration discovery mechanism. It works without internet access, without DHT bootstrap nodes, without anything. Turn on two nodes on the same Wi-Fi, and they find each other.

### Layer 2: Kademlia DHT (Internet)

When a node connects to any peer (via mDNS or manual dial), the peer's address feeds into Kademlia's routing table. From there, DHT lookups work transitively — you can find peers you've never directly connected to.

The Kademlia DHT is the global discovery layer. It works across the internet, through NATs (with relay support), and scales to millions of peers.

### Layer 3: Manual Dialing

For cases where automatic discovery isn't enough (different networks, firewalled peers), the `dial` command connects directly to a peer's multiaddress:

```bash
canopee dial /ip4/192.168.1.100/tcp/4001/p2p/12D3KooWABCD...
```

### Layer 4: Relay + Hole Punching

Peers behind restrictive NATs can use a relay node as a stepping stone:

```bash
canopee listen-via-relay /ip4/relay-server/tcp/4001/p2p/12D3KooWRELAY...
```

The swarm requests a circuit reservation on the relay. Once connected, DCUtR (Direct Connection Upgrade through Relay) attempts to establish a direct connection by exchanging observed addresses through the relay.

## Object Exchange

Objects are exchanged using a custom CBOR-encoded request/response protocol:

```
Client → Server: ObjectRequest::GetObject(ObjectId)
Server → Client: ObjectResponse::Object(ExportBundle) | ObjectResponse::NotFound
```

The protocol is simple: the client asks for an object by ID, the server responds with the complete `ExportBundle` (object + signature + public key). The client verifies the bundle before accepting it.

Object exchange is built on libp2p's request-response behavior, which handles connection management, backpressure, and error reporting.

## Mutable Records: DHT Operations

Mutable records use Kademlia's record operations:

### Put Record

```rust
network.put_record(key, value)
```

Overwrites the DHT entry outright. The caller must ensure the key is derived from their own identity and the value is self-verifying. `NetworkManager` does no validation — it trusts the caller.

### Get Record

```rust
network.get_record(key).await → Result<Option<Vec<u8>>>
```

Fetches the record from the DHT. Returns `None` if no record exists or the query times out.

The pending query uses Kademlia's `QueryId` bookkeeping, resolved in the `OutboundQueryProgressed` event. If the caller's oneshot is dropped (e.g., timeout), the query continues in the background but the result is discarded.

## Pub/Sub: Gossipsub

Gossipsub provides topic-based publish/subscribe messaging:

```rust
network.subscribe("my-topic") → broadcast_receiver
network.publish("my-topic", data)
```

Gossipsub is used for real-time communication — chat messages, notifications, live updates. Messages are propagated through the gossip mesh, reaching all subscribers of a topic.

The `Subscribe` command in the node protocol hijacks the connection: after the initial `Subscribed` frame, the node pushes `PubSub` frames until the client disconnects. Dropping the connection is the unsubscribe mechanism — there is no explicit `Unsubscribe` command.

**Important**: publishing fails with `InsufficientPeers` if nobody is subscribed. Gossipsub is not a broadcast-to-all mechanism; it requires a mesh.

## Announcing and Finding Providers

For objects shared via the DHT:

```rust
network.announce(object_id)      // register as provider
network.find_providers(object_id) // discover providers
```

Announcing creates a DHT provider record mapping the object ID to the announcing peer. Finding providers queries the DHT for that mapping. Multiple peers can announce the same object, creating redundancy.

## The ObjectProvider Trait

The network layer is decoupled from storage through the `ObjectProvider` trait:

```rust
#[async_trait]
trait ObjectProvider: Send + Sync + 'static {
    async fn get_object(&self, id: &ObjectId) -> Option<ExportBundle>;
}
```

The swarm uses this trait to serve objects to requesting peers. The runtime implements it on behalf of the storage layer (the network crate does depend on `canopee-storage` for the `ExportBundle`/`ObjectId` types, but the serving decision — shared set vs. cached — is injected via this trait). This boundary allows the serving policy to be tested independently of the swarm.

## Design Decisions

### Every Node is a Relay

Canopee does not have a relay tier. Every node runs the relay server behavior. This means:

- **No infrastructure**: peers behind NATs can always find a relay
- **Small constant cost**: every node relays a small amount of traffic for others
- **Trade-off**: there is no way to opt out of relaying

This is a deliberate simplicity-over-efficiency choice. The relay traffic for any individual node is small, and the elimination of infrastructure is worth the cost.

### Server Mode for Kademlia

The Kademlia behavior runs in **server mode** by default. This is essential — in client mode, a node won't respond to DHT queries from other nodes, which silently breaks discovery for two-client scenarios. This was discovered the hard way (documented in the `two_nodes_dial_and_discover_via_kad` test).

### Bootstrap

New nodes dial a set of default bootstrap addresses on startup to seed their Kademlia routing table. A default bootstrap relay is hardcoded into the network crate, overridable via the `CANOPEE_BOOTSTRAP_ADDRS` (replace) and `CANOPEE_BOOTSTRAP_ADDRS_PREPEND` (prepend) environment variables. mDNS handles local discovery with zero configuration; for internet-wide discovery the bootstrap dial gives the DHT its first entries, after which any connection feeds Kademlia transitively.

## Testing the Network

The network layer includes integration tests that spin up real swarms:

```bash
cargo test -p canopee-network --test-threads=1
```

Tests cover:
- Dial and discover via Kademlia
- DHT announce → find providers → fetch object
- Gossipsub publish → subscribe → receive messages

The `--test-threads=1` flag is required to avoid port contention between concurrent test swarms.
