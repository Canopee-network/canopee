# Networking

Canopee builds its P2P layer on **libp2p**. There are no servers, no central
indexes, and no accounts managed by anyone else — your node discovers peers,
finds providers on the DHT, and serves objects to anyone allowed.

The swarm lives in `canopee-network` (crates/canopee-network/src/), wired
together in `behaviour.rs` and driven by the event loop in
`manager/event_loop.rs`.

## Behaviours in the swarm

| Behaviour | Job |
|---|---|
| `mdns` | zero-config LAN discovery (off by default for embedded/app-root nodes) |
| `kademlia` | DHT: provider records, username/device registries, pointer lookup |
| `identify` | tell peers your supported protocols + agent string |
| `gossipsub` | pub/sub messaging (`publish` / `subscribe`) across the network |
| `request-response` | fetching objects from peers (the object exchange protocol) |
| `autonat` + `dcutr` | NAT traversal: detect if you're behind a NAT, then punch a direct connection through it |
| `relay` | circuit-relay: when direct connection is impossible, route through a relay node |
| Canopee pairing/object request handlers | the two custom application protocols |

## Discovery

1. **LAN — mDNS**: on a shared network, nodes publish their
   `PeerId` and address; peers appear automatically. Nothing to configure.
   Disabled with `CANOPEE_MDNS=0` or in app-root mode.
2. **WAN — the DHT (Kademlia)**: nodes join the global kademlia overlay
   through the configured bootstrap addresses. Bootstrap defaults to the
   public relay/foundation nodes in `bootstrap_addrs()`; you can override with
   `CANOPEE_BOOTSTRAP_ADDRS` or prepend with `CANOPEE_BOOTSTRAP_ADDRS_PREPEND`
   (see [Environment reference](../reference/environment.md)).

## Providers, not requirements

Objects are stored locally on the nodes that created (or imported) them. When
you `share` an object, your node announces itself as a **provider** of that
object id on the DHT (`announce`, via the runtime's `announce`).

- Anyone interesting in the object runs `find-providers <id>` and gets a list
  of peer ids currently serving it.
- Fetching pulls it into the requester's local storage + cache, so even a
  *leaf* node becomes a provider after it has fetched once (`fetch_object` /
  `find_providers`, `cached_bytes` LRU bounds the cached set).

This is how content gets *replicated*: the act of fetching makes you a source
for everyone else. It's how apps served over the network get distributed —
see the [Publishing guides](../guides/publishing-apps.md) and
[Content caching](../guides/content-caching.md).

## Pointers and registries over the DHT

Several mutable Kademlia records are keyed globally (see
[Objects & pointers](objects.md)):

```
username:<name>   → verified owner identity        (unique name registry)
device:<peer-id>  → verified owner identity        (device registry)
(owner, name)     → pointer to latest object       (resolved via pointer protocol)
```

Because these records are *signed by the owner identity*, a corrupted or
stale DHT answer simply fails verification — the record model never relies on
the swarm being honest.

## NAT traversal

Home nodes sit behind NATs. Three mechanisms, in preference order:

1. **autonat** tells the node whether it's publicly reachable.
2. If not, it asks relay nodes for a circuit-relay reservation — every node
   listens on `.../p2p-circuit` so it's *always* connectable via relay.
3. **dcutr** then attempts to *upgrade* the relayed connection into a direct
   hole-punched one. Consumers set up a reservation on a relay once and learn
   the relayed peer id via `relay_peer_id_from_circuit_addr`; direct
   connections are established opportunistically.

The CLI surface: `canopee listen-via-relay <relay-addr>`,
`canopee relay-status`, `canopee peers`. The relay protocol runs on port
`4001/tcp` on the foundation relay node (see
[Bootstrap & relay deployment guide](../guides/bootstrap-nodes.md)).

## The object-exchange request protocol

Beyond DHT provider discovery, a direct `request-response` protocol lets a
peer ask a specific peer for a specific object id. This is what
`canopee fetch <peer> <name>` uses: resolve the peer's shared entry name, then
pull the object straight from them. Request-style delivery also powers
`sync` (each device asks the owner's other devices for the owner's records).

## Pub/sub (gossipsub)

For sending live messages, not static content:

- `canopee publish <topic> <data>` or SDK `publish(topic, data)`;
- `canopee subscribe <topic>` or SDK `subscribe(topic) → Subscription`;
- any node that has the feed subscription can relay.

Pub/sub is a distinct channel from object exchange: messages are ephemeral
and unaddressed (everyone on the topic sees them). The
[chat guide](../guides/chat-between-peers.md) uses this for live messages and
blends it with object pointers for chat history.