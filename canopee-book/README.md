# The Canopee Book

A complete presentation and documentation of Canopee — what it solves, how to use it, its philosophy, and deep technical explanations of its inner workings.

## Reading Order

The book is organized from concepts to implementation:

### Part I: Concepts

1. **[Introduction](01-introduction.md)** — What Canopee is, the problem it solves, core principles
2. **[The Object Model](02-object-model.md)** — Content-addressed, signed, immutable objects
3. **[Identity and Cryptography](03-identity-cryptography.md)** — Ed25519 keys, X25519 DH, domain separation
4. **[Configuration and Path Resolution](04-configuration.md)** — Split-root model, path getters, no-config philosophy
5. **[Mutable Records](05-mutable-records.md)** — Signed pointers to immutable objects, the sharing gate

### Part II: Implementation

6. **[Networking](06-networking.md)** — libp2p swarm, discovery, DHT, pub/sub, relay
7. **[The Runtime](07-runtime.md)** — Tying identity, storage, and network together
8. **[The Node Daemon](08-node-daemon.md)** — Unix socket server, wire protocol, 44 commands
9. **[The SDK](09-sdk.md)** — Application client library
10. **[The CLI](10-cli.md)** — Command-line interface reference

### Part III: Features

11. **[The Gateway](11-gateway.md)** — WebSocket bridge for browsers
12. **[Application Distribution](12-app-distribution.md)** — Manifests, publishing, serving
13. **[Security Model](13-security.md)** — Trust boundaries, verification, threat model
14. **[End-to-End Encryption](14-e2e-encryption.md)** — X25519 DH, ChaCha20-Poly1305, the chat pattern

### Part IV: Building

15. **[Building Applications](15-building-apps.md)** — Patterns, Tauri integration, testing
16. **[Deployment](16-deployment.md)** — Development, relay nodes, NAT traversal
17. **[Comparison](17-comparison.md)** — vs IPFS, Hypercore, SSB, client-server

### Part V: Discovery

18. **[Peer Profiles](18-peer-profiles.md)** — The profile system, interest profiles, what's implemented and what's missing
19. **[Semantic Search](19-semantic-search.md)** — Interest-based similarity search, the `interest-discovery` reference implementation

### Part VI: Stories

20. **[User Stories](20-user-stories.md)** — 22 scenarios demonstrating every feature
21. **[Roadmap and Known Limitations](21-roadmap.md)** — What's planned, what's missing

## Quick Start

```bash
# Initialize
canopee init

# Start the node
canopee start

# Store something
canopee put ./myfile.txt

# See your identity
canopee identity

# List connected peers
canopee peers
```

## For Developers

```rust
// Embed Canopee in your application
let config = Config::new()
    .with_app_root(app_data_dir)
    .with_user_root(user_dir)
    .with_mdns(false);

let runtime = Runtime::open_with_config(config).await?;

// Store objects
let object_id = runtime.put(data).await?;

// Share with peers
runtime.share_object("My File", &object_id, Some("my-app".to_string())).await?;

// Fetch from peers
let bundle = runtime.network
    .get_object(peer_id, object_id)
    .await?;
```

## License

This book documents the Canopee project. See the project's LICENSE file for terms.
