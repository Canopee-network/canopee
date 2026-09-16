# Chapter 17: Comparison with Other Systems

## Canopee in the Ecosystem

Canopee sits in a specific niche among decentralized systems. Understanding how it compares to IPFS, Hypercore, Secure Scuttlebutt, and traditional client-server helps clarify its design choices.

## vs IPFS / IPNS

### Similarities
- Content-addressed object storage
- DHT-based peer discovery (Kademlia)
- Object exchange protocols
- Mutable pointers (IPNS vs Canopee records)

### Differences

| Aspect | IPFS | Canopee |
|--------|------|---------|
| **Scope** | Global filesystem | Embedded app data layer |
| **Infrastructure** | Bootstrap nodes, dedicated relays | Every node is a relay |
| **Mutable records** | IPNS (slow, centralized signing) | Signed records (fast, local-first) |
| **Identity** | PeerID (key-only, no user model) | IdentityId + usernames + profiles |
| **App distribution** | Not built-in | AppManifest + AppPointer + serving |
| **Encryption** | None built-in | X25519 DH (app-layer E2E) |
| **Embeddability** | Heavy, requires daemon | Designed for in-process embedding |
| **Configuration** | Extensive config files | Zero config files |
| **Scope** | Store everything | Store app-relevant data |

### Key Distinction

IPFS is a global, general-purpose filesystem. Canopee is a local-first, app-specific data layer. IPFS wants to be the hard drive of the internet. Canopee wants to be the data backbone of desktop applications.

IPNS (InterPlanetary Name System) is IPFS's mutable record system, but it's slow (requires centralized signing or DHT publication with long propagation) and doesn't provide the application-layer semantics (profiles, contacts, home indexes) that Canopee does.

## vs Hypercore / Hyperswarm

### Similarities
- Peer-to-peer networking
- Content-addressed data
- Designed for application developers
- Local-first

### Differences

| Aspect | Hypercore | Canopee |
|--------|-----------|---------|
| **Data model** | Append-only logs | Content-addressed objects |
| **Networking** | Hyperswarm (DHT + DNS) | libp2p (Kademlia + mDNS) |
| **Identity** | Public key only | Full identity with profiles |
| **Mutable records** | Feed heads | Signed (owner, name) records |
| **App distribution** | Not built-in | Full app manifest + serving |
| **Language** | JavaScript (JavaScript-centric) | Rust (embedded in any language) |
| **Encryption** | Optional per-feed | Application-layer E2E |

### Key Distinction

Hypercore is a peer-to-peer data structure (append-only logs) with networking. Canopee is a complete data platform with identity, storage, records, networking, and app distribution. Hypercore gives you a building block; Canopee gives you the whole floor.

Hypercore's append-only log model is powerful for streaming data (video, audio, time series) but less natural for the "point to latest version" pattern that profiles, contacts, and app manifests need.

## vs Secure Scuttlebutt (SSB)

### Similarities
- Peer-to-peer
- Identity-centric (each peer has a keypair)
- Social features (contacts, profiles)
- Local-first

### Differences

| Aspect | SSB | Canopee |
|--------|-----|---------|
| **Data model** | Append-only message logs | Content-addressed objects |
| **Social graph** | Follow/block protocol | ContactList + HomeIndex |
| **Pub/sub** | Rooms (gossip protocol) | Gossipsub (libp2p) |
| **App distribution** | Not built-in | Full app manifest + serving |
| **Discovery** | Pub (gossip-based DHT) | mDNS + Kademlia |
| **Transport** | TCP + TLS | libp2p (TCP, relay, hole punching) |
| **Mutable data** | Only via append | Signed mutable records |

### Key Distinction

SSB is a social protocol — it's designed for social networking, blogging, and community applications. Canopee is a data infrastructure — it's designed for any application that needs local-first, peer-to-peer data storage.

SSB's append-only log model means "editing" requires appending a new message that supersedes the old one. Canopee's mutable records are designed for exactly this — a record that points to the latest version of an object.

## vs Traditional Client-Server

| Aspect | Client-Server | Canopee |
|--------|---------------|---------|
| **Data location** | Server | User's device |
| **Availability** | Server-dependent | Peer-to-peer |
| **Identity** | Account per service | Self-sovereign keypair |
| **Data portability** | Export APIs | Built-in (ExportBundle) |
| **Privacy** | Server sees everything | Local-first, E2E optional |
| **Cost** | Server infrastructure | Minimal (one bootstrap relay) |
| **Offline** | No access | Full access to local data |

### Key Distinction

Client-server is the dominant model because it's simple to build and deploy. Canopee trades that simplicity for data ownership and offline capability. The question isn't "which is better" — it's "which trade-offs matter for this application."

## Why Canopee Exists

None of the existing systems perfectly fit the niche of "embedded, identity-centric, app-aware, zero-infrastructure peer-to-peer data layer":

- **IPFS** is too heavy and too general
- **Hypercore** is too low-level and too JavaScript-centric
- **SSB** is too social-protocol-specific
- **Client-server** doesn't solve the data ownership problem

Canopee fills this gap with:

1. **Embeddability**: runs inside the application process, no daemon
2. **Identity**: full identity with profiles, contacts, and usernames
3. **App awareness**: built-in app manifest, distribution, and serving
4. **Minimal infrastructure**: every node is a relay; the only fixed piece is a default bootstrap relay used for DHT seeding (overridable via env var)
5. **Records**: mutable pointers designed for application data, not just log heads
6. **Sharing**: opt-in sharing with the HomeIndex gate

Canopee is not trying to replace IPFS or Hypercore. It's building something different — a data layer that makes peer-to-peer applications as easy to build as client-server ones.
