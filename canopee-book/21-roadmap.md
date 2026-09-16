# Chapter 21: Roadmap and Known Limitations

## What's Implemented

Canopee is a working system. The following features are implemented and tested:

- **Identity**: Ed25519 keypair generation, loading, encrypted at rest
- **Object Storage**: Content-addressed, flat-file, verified on read/write
- **Mutable Records**: Signed, versioned pointers with local cache + DHT
- **Networking**: libp2p swarm with mDNS, Kademlia DHT, Gossipsub, relay, DCUtR
- **Node Daemon**: Unix socket server with 44 commands
- **SDK**: Typed client library with streaming subscription
- **CLI**: Full command-line interface for all operations
- **Gateway**: WebSocket bridge for browsers with session tokens
- **App Distribution**: Manifest publishing, resolution, and local HTTP serving
- **Usernames**: Human-readable aliases with spoof-verified resolution
- **Multi-Device Identity**: per-device keys, LAN pairing, device lists, cross-device record sync
- **E2E Encryption**: X25519 DH key agreement (demonstrated in chat app)
- **Interest Discovery**: similarity search over peer interests, implemented purely on the SDK (`interest-discovery`, see Chapter 19)
- **Embedded Runtime**: In-process node for desktop applications (Tauri demo)

## What's Planned

### Certificate-Authenticated / End-to-End Transport (Critical)

Wire traffic is already encrypted: the libp2p transport uses **Noise** on both TCP and relay connections, and gossipsub messages are signed. The genuinely unstarted piece is *certificate-authenticated, end-to-end* transport. Noise is peer-to-peer between directly-connected nodes — a relay sees the plaintext it forwards, and there is no PKI binding a key to a name. Adding TLS-with-certificates (or an equivalent) would provide:

- **End-to-end transport security**: protection across relays, not just hop-by-hop
- **Certificate-based trust**: optional integration with existing PKI

The remaining sub-item:
- **Loopback TLS**: encrypt the gateway WebSocket (trivial, local scope) — currently plaintext loopback

### Content Encryption

Objects are currently stored and transmitted in plaintext. Content-level encryption would allow:

- **Private objects**: encrypted with a key only the owner knows
- **Shared encryption**: encrypt for specific recipients using their DH keys
- **Encrypted storage**: protect data at rest on disk

The chat app demonstrates the pattern (X25519 + ChaCha20-Poly1305), but it's not generalized to the storage layer.

### Bootstrap (Implemented)

~~New nodes currently need a manual `dial` or `listen-via-relay` to join the DHT.~~ Automatic bootstrap is **implemented**: a default bootstrap relay is hardcoded in the network crate, dialed on every startup to seed the Kademlia routing table, with `CANOPEE_BOOTSTRAP_ADDRS` / `CANOPEE_BOOTSTRAP_ADDRS_PREPEND` env overrides. Zero-configuration internet-wide discovery works today.

### SPA Hosting Improvements

The app server already has an SPA fallback (route-shaped paths serve `index.html`; asset-shaped misses 404), so deep links work without hash-based routing. Remaining rough edges:

- Font and icon asset handling could be improved (common font/icon MIME types are supported, but coverage isn't exhaustive)

### Structured Logging

The node and CLI use `println!` liberally (the node even prints every stream/response), though the network crate already logs through `tracing`. A consistent structured-logging layer would enable:

- Production monitoring
- Log aggregation
- Performance profiling

### Relay Tier

Every node currently relays traffic for others. A future improvement could add:

- **Relay opt-out**: nodes that don't want to relay
- **Relay reputation**: score relays by reliability
- **Relay selection**: prefer fast, stable relays

### P2P App Caching Improvements

The distributed cache (re-serving fetched objects) is partially built. Already implemented:

- **Eviction policy**: least-recently-served eviction over a configurable cap (`CANOPEE_CACHE_MAX_MB`, default 256 MiB), swept every 30s in the node daemon — owned objects are never evicted
- **Announce-on-fetch**: done in the CLI app-fetch path (fetched app assets are announced as providers)

Still open:

- **Announce-on-fetch in generic `Runtime::fetch_object`** (only the CLI app path announces today)
- **Cache warming**: proactively fetch popular objects

### Replay Protection

Mutable records currently have **no** replay protection — versions are informational, and nothing compares a record's `published_at` against a previously-seen value, so an old (validly signed) record replayed in place of a newer one is accepted. Adding timestamp comparison or epoch-based invalidation would prevent:

- Old record versions being re-published
- DHT records being replayed after revocation

### Semantics: Interest Discovery and Search

The core has no search; but Chapter 19 documents a *working, tested* application-layer design: `interest-discovery` proves similarity search over peer interests using deterministic hashed-character-n-gram embeddings, LSH bucket keys published as virtual DHT provider records, and verify-on-the-way-back scoring. Core-level opportunities this opens:

- **Richer structured profile fields** — let users publish bio/tags/status in a first-class, indexable record beyond `display_name`/`dh_public_key`/`avatar` (the `InterestProfile` is the app-layer proof of the pattern)
- **Local full-text index** of one's own objects (metadata + names) to close the O(n) `list()` gap
- **`unannounce` for applications** so edited profiles stop leaving stale bucket announcements until restart
- **Stronger embeddings (research tier)** — transformer-based embeddings for synonymy-aware similarity; the current hashed n-grams capture morphology, not synonyms

### Offline Message Delivery

The chat application currently requires both peers to be online. Offline delivery would require:

- **Message queuing**: store encrypted messages for offline peers
- **Delivery on connect**: send queued messages when the peer comes online
- **End-to-end encryption**: messages must be encrypted for the recipient, not the network

## Known Limitations

### Peer.identity Population

~~The `Peer` struct's `identity` field is always `None`.~~ The `identity` field *is* populated: it is set to `Some(IdentityId)` on connection establishment and on Identify events, and enriched with username/display_name via `enrich_peers`. (It starts as `None` in `Peer::new` and is filled in as the connection is identified.)

### NodeState.peers is Unused

The `peers` field in `NodeState` is a placeholder. The live peer list lives in `NetworkManager`, not in the persisted state.

### No Rate Limiting

There is no built-in rate limiting for any operation. A malicious peer could flood a node with object requests, DHT queries, or pub/sub messages.

### No Revocation

There is no mechanism to revoke an identity or invalidate a compromised key. The identity is the key — lose it, lose everything.

### Timestamp Metadata is Self-Reported

Object metadata (`created_at`) is set by the creating node and not verified by readers. A node could backdate or future-date objects.

## Contributing

Canopee is an open project. Contributions are welcome in these areas:

1. **TLS integration** — the highest priority item
2. **Content encryption** — generalize the chat pattern to the storage layer
3. **Testing** — more integration tests, especially for NAT traversal
4. **Documentation** — tutorials, examples, API documentation
5. **Performance** — profiling, optimization, benchmarking
6. **Security review** — audit the cryptographic implementations

## The Vision

Canopee aims to make peer-to-peer applications as easy to build as client-server ones. The current implementation demonstrates that this is possible — the chat app proves it in practice.

The roadmap focuses on closing the gaps between "working demo" and "production infrastructure": TLS, encryption, bootstrap, logging, and reliability. Each step makes Canopee more practical for real-world applications.

The end goal is a world where applications don't need servers. Where data lives on user devices. Where identity is self-sovereign. Where the network is the sum of its participants, with no hidden infrastructure.

Canopee is one step toward that world.
