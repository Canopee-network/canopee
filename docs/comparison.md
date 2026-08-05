# How Canopee compares to similar projects

This is a conceptual doc, not a tutorial — it exists to answer "how is this
different from IPFS/X" honestly, grounded in what's actually built here
(content-addressed signed objects, a libp2p swarm, Kademlia DHT, gossipsub,
app manifests) rather than a generic decentralization pitch. Read
[`networking-for-beginners.md`](networking-for-beginners.md) first if
you're not already familiar with the underlying concepts (DHT, relays,
content addressing) — this doc assumes you know what those words mean and
focuses on comparison, not explanation.

## Similar existing projects

| Project | Core model | Closest overlap with Canopee |
|---|---|---|
| **IPFS** | Content-addressed blocks (CIDs), Kademlia DHT, Bitswap for transfer, libp2p under the hood | Almost identical primitives — content addressing + DHT + libp2p is literally IPFS's architecture. Canopee's `ObjectId`/`find_providers`/`fetch_object` map closely to IPFS's CID/DHT-provider-record/Bitswap. |
| **IPFS + IPNS** | IPNS adds mutable names on top of immutable CIDs (signed records published to the DHT, conceptually similar to DNS) | Directly comparable to [`AppPointerRecord`](app-manifests.md#resolving-by-name-app-pointers) — same problem (a stable name resolving to changing immutable content), same DHT-record mechanism. |
| **Hypercore / Beaker Browser** | Append-only signed logs (not blob-addressed), `hyper://` URLs, a purpose-built browser exposing a `beaker.hyperdrive` JS API to pages | The closest real-world precedent for the "Canopee browser" idea discussed in [`publishing-vs-building-apps.md`](publishing-vs-building-apps.md#where-they-meet) — a browser that bridges published content and a scripting API into one surface. Worth studying directly if that idea is ever built. |
| **Secure Scuttlebutt (SSB)** | Append-only per-identity logs, epidemic gossip-based replication, offline-first, no DHT | Closer to Canopee's *identity* model (one keypair, signed feed of activity) than to its object storage — no content-addressing or DHT lookups; replication is gossip-based rather than structured, a different transport philosophy entirely. |
| **libp2p (bare)** | Just the networking toolkit — swarm, Kademlia, gossipsub, relay/hole-punching, request-response | Not a competing project — it's the literal dependency Canopee is built on ([`canopee-network`](../crates/canopee-network/README.md)). Listed because IPFS's newer implementation (Kubo) and Iroh below are themselves libp2p (or libp2p-adjacent) consumers too — Canopee sits at roughly the same architectural layer as a from-scratch libp2p application. |
| **Iroh** | Newer Rust-native P2P toolkit: content-addressed blobs, QUIC-based direct connections, explicitly positioned as "simpler than IPFS, Rust-first" | The most directly comparable project technically — same language, same "content-addressed blob transfer over a P2P swarm" pitch, further along and more actively maintained. The closest thing to a real technical peer on this list. |
| **Nostr** | Signed events relayed through simple, non-P2P relay servers; no DHT, no content addressing — identity + signed messages is the whole model | Functionally close to what [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md) builds (identity-signed messages over pub/sub), but architecturally opposite: Nostr relays are simple servers you connect to, not peers in a DHT-routed swarm. |
| **Matrix** | Federated (not peer-to-peer) chat protocol — servers replicate rooms between each other | Not P2P at all; included because it's the most common answer to "how do I build decentralized chat," and the contrast is instructive — Matrix federates between servers, Canopee has no server tier by design (see [`canopee-network/README.md`](../crates/canopee-network/README.md)'s "every node is a relay" note). |

## Where Canopee stands out

Three real design differences, not just "it's also P2P":

### 1. One identity, every capability — no protocol boundary between subsystems

In IPFS, content addressing (CIDs), naming (IPNS), and pub/sub evolved as
related but somewhat separate subsystems layered onto the stack over time.
Canopee's single [`Identity`](../crates/canopee-identity/README.md) backs
*everything* directly: object signing, the libp2p `PeerId`, gossipsub
message authorship, and `AppPointerRecord` signatures are all the same
keypair, deliberately — see `canopee-identity/README.md`'s design notes on
why one keypair serves both the network address and the signing identity.
There's no separate "login" or credential per feature.

### 2. App manifests + app pointers as an opinionated, first-class publishing pipeline

IPFS gives you CIDs and IPNS as building blocks and leaves "how do I
structure and deploy a publishable app with an entrypoint and a stable
name" to a separate tool (`ipfs-deploy`, Fleek, or your own scripts).
Canopee bakes [`AppManifest`/`AppPointerRecord`](app-manifests.md) directly
into the base CLI as one coherent, tested, documented flow: `app-manifest`
→ announce → `open` (by id or by `--owner`/`--name`) is a single command
sequence here, not a pattern you assemble from primitives yourself.

### 3. Every node relays by default — no separate infrastructure tier

IPFS distinguishes ordinary nodes from relay/bootstrap infrastructure in
practice (public gateways, dedicated bootstrap peers most deployments rely
on). Canopee states the opposite as explicit policy in
[`canopee-network/README.md`](../crates/canopee-network/README.md):
*every* node ships with relay behavior turned on by default, and any
publicly reachable node can relay for a NAT'd one with no opt-in step.
The tradeoff — every node pays a small relay-serving cost even if it never
needs relaying itself — is a deliberate, stated choice, not an oversight.

## Where Canopee is honestly behind, not ahead

This isn't a "Canopee wins" comparison. Concretely, today:

- **No encryption anywhere** — see
  [`security-considerations.md`](security-considerations.md) for the full
  list (message content, transport TLS, key-at-rest). IPFS/Iroh have the
  same gap in different forms, but it's still a real gap here, not a
  differentiator.
- **No bootstrap node list yet** — [`bootstrap-nodes-tutorial.md`](bootstrap-nodes-tutorial.md)
  scopes the work; IPFS ships with a working default bootstrap list today.
- **Far less battle-tested and adopted.** IPFS has years of production
  usage, tooling, and a large ecosystem (pinning services, gateways,
  IPFS-aware browsers). Iroh, while newer, is more actively developed and
  further along in maturity than this repo. Canopee is explicitly a
  reference implementation — see the root [`README.md`](../README.md)'s
  "Status" section — not a production system.
- **A much smaller feature surface overall.** No pinning services, no
  public gateways, no wide client ecosystem, no CDN-style edge caching
  (see [`roadmap-hosting-replacement.md`](roadmap-hosting-replacement.md)
  for what "closing the gap with a real hosting provider" would actually
  take).

The comparison above is about *design choices that differ*, not about
Canopee being more mature or more capable than the projects it's compared
to. If you're evaluating "which of these should I actually build on
today," maturity and ecosystem size should weigh heavily — the
differentiators above are what make Canopee interesting to study or
extend, not necessarily what make it production-ready to depend on right
now.
