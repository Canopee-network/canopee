# Comparison with similar projects

How Canopee relates to other decentralized systems — honestly. For the
conceptual vocabulary (content-addressing, DHT, relays) see
[Networking concept](../concepts/networking.md).

## Similar projects

| Project | Core model | Closest overlap with Canopee |
|---|---|---|
| **IPFS** | Content-addressed blocks (CIDs), Kademlia DHT, Bitswap, libp2p | Almost identical primitives. `ObjectId`/`find_providers`/`fetch_object` map closely to IPFS's CID/DHT-provider-record/Bitswap. |
| **IPFS + IPNS** | IPNS: mutable names over immutable CIDs (signed DHT records) | Directly comparable to `AppPointerRecord` / `(owner, name)` pointers — same problem, same DHT-record mechanism. |
| **Hypercore / Beaker Browser** | Append-only signed logs, `hyper://` URLs, browser-stdlib API | Closest precedent for the "Canopee browser" idea in [Publishing vs building apps](../guides/publishing-apps.md). |
| **Secure Scuttlebutt (SSB)** | Per-identity signed feeds, gossip replication, no DHT | Closer to Canopee's *identity* model than to its object store; a different transport philosophy. |
| **libp2p (bare)** | The networking toolkit: swarm, Kademlia, gossipsub, relay, request-response | Not a competitor — the dependency Canopee builds on. Canopee sits at the same layer as a from-scratch libp2p app. |
| **Iroh** | Rust-native P2P toolkit: content-addressed blobs over QUIC | The most technically comparable project — same language, same "content-addressed blob transfer over a swarm" pitch, further along. |
| **Nostr** | Signed events relayed through simple non-P2P servers; no DHT | Functionally close to what the [chat tutorials](../guides/README.md) build (signed messages over pub/sub), architecturally opposite. |
| **Matrix** | Federated chat — servers replicate rooms between each other | Not P2P at all; useful as the contrast that Canopee deliberately has no server tier. |

## Where Canopee differs

1. **One identity backs everything.** Object signatures, record pointers,
   app pointers, and `canopee://identity/<id>` all come from a single account
   identity; a per-device `DeviceKey` provides the network `PeerId`, keeping
   "who signed this" and "which device is serving it" independent — several
   devices of one identity can be online at once (see
   [Identity concept](../concepts/identity.md)).
2. **App manifests + app pointers are first-class.** `app-manifest` →
   announce → `open` (by id or `--owner`/`--name`) is one coherent flow in
   the base CLI, where IPFS tangles it into separate tools (`ipfs-deploy`,
   Fleek, your own scripts).
3. **Every node relays by default.** Canopee's explicit policy: no separate
   relay/bootstrap infrastructure tier. Any publicly reachable node can
   relay for a NAT'd one with no opt-in step (see
   [Networking concept](../concepts/networking.md)).

## Where Canopee is honestly behind

- **No E2E encryption of content by default.** Objects are signed, not
  encrypted (the transport is Noise-encrypted hop-by-hop; key-at-rest is
  opt-in via `CANOPEE_IDENTITY_PASS`). See
  [Security concept](../concepts/security.md).
- **A minimal, hardcoded bootstrap list** (single entry). IPFS ships a
  richer community-maintained default. A DNS-based discovery design exists
  in the archive, unbuilt.
- **Less battle-tested and adopted.** Reference implementation, not a
  production system—see the status note in the root `README.md`.
- **A much smaller feature surface**: no pinning services, no public
  gateways, no CDN-style edge caching. What closing each gap would take is
  tracked in [Roadmap](roadmap.md).

The comparison is about *design choices that differ*, not maturity. If
you're deciding what to build on today, ecosystem maturity should weigh
heavily; the differentiators above are what make Canopee interesting to
study or extend.