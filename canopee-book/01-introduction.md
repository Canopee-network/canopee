# Chapter 1: Introduction

## What is Canopee?

Canopee is a decentralized, identity-centric content-addressed object store with a peer-to-peer network, designed to be embedded directly inside desktop applications. It is not a blockchain. It is not a server platform. It is an infrastructure layer that gives every application its own node on a peer-to-peer network — with no servers, no bootstrapping clusters, and no central authority.

Every device is a peer. Every peer owns signed, immutable objects. Mutable "records" point at the latest version of those objects. That's the whole model.

## The Problem

Modern applications are built on a client-server architecture that creates several fundamental problems:

**Data ownership is an illusion.** When you store a file in a cloud service, you don't own it — you rent space on someone else's server. The service can change its terms, go down, or disappear entirely. Your data goes with it.

**Identity is fragmented.** Every application invents its own authentication system. You have a different username, password, and profile for every service. There is no portable, self-sovereign identity that you control.

**Applications are islands.** Data cannot flow between applications without APIs, middleware, and server-side glue code. Two apps made by the same developer, running on the same machine, cannot share data without a backend.

**Infrastructure is expensive and fragile.** Deploying a peer-to-peer application still requires servers for signaling, relaying, and coordination. The "decentralized" application has a single point of failure in its infrastructure layer.

## The Canopee Answer

Canopee inverts the traditional model. Instead of applications owning servers, **applications own nodes**. A Canopee node is a lightweight, embedded peer on a libp2p network that:

- Stores objects locally on disk, content-addressed and cryptographically signed
- Discovers other peers on the local network automatically (mDNS) or across the internet (DHT)
- Exchanges objects directly with peers — no server in the middle
- Maintains mutable records that point to the latest version of objects, signed by the owner
- Relays traffic for other peers — every node is a relay, eliminating infrastructure

The node runs inside the application process. There is no daemon to install, no server to configure, no port to forward (unless you want to). The application and the network are one thing.

## Core Principles

### State is per-app, data is per-user

Application state — the socket file, exports, cache, runtime state — lives under each application's own root directory. Identity, the object store, records, and aliases live in a shared user root (`~/.canopee`). Two applications by the same person share one identity and one object store. Two runtimes with different roots are fully isolated.

This means your identity and data follow you across applications. Install a new Canopee-powered app, and it immediately has access to your identity, your contacts, your profile. No re-registration, no data migration.

### Nothing is shared by default

Every object is private unless you explicitly share it. An object is served to the network only if:

1. It has been announced to the DHT, or
2. It is listed in your `HomeIndex` with `shared: true`

Objects cached from peers are always re-served — that re-hosting *is* the distributed cache. But your own objects? They stay on your disk until you decide otherwise.

### Objects are immutable and self-verifying

An object's identity is its content: `ObjectId = sha256(bincode(payload))`. The object includes a signature from the owner's private key. Both the content hash and the signature are verified on every read and every write. If someone tampers with an object's bytes, the hash changes and the object becomes a different object. If someone forges an object, the signature fails verification.

This makes the object store inherently tamper-evident. There is no way to inject a corrupted or forged object without detection.

### Signing happens in the node, never the client

Only the node holds the identity's private key. SDK clients and CLI send raw, unsigned data. The node signs it, bumps the version number, and repoints the record. This makes record authorship unspoofable from the outside — no client code can forge a signature, and no intercepted message can be replayed as a new version.

### Minimal infrastructure

Every node is a libp2p relay — any peer can relay traffic for any other. Browsers reach their own node through a loopback-only WebSocket gateway. The only fixed infrastructure is a small set of default bootstrap addresses (a hardcoded bootstrap relay, overridable via the `CANOPEE_BOOTSTRAP_ADDRS` environment variable) that new nodes dial on startup to seed their Kademlia routing table. There is no DNS server, no signaling server, and no consensus layer beyond that initial rendezvous.

## What Canopee Is Not

- **It is not a blockchain.** There is no consensus, no mining, no blocks. Records are signed by their owner and verified by their readers. Trust is direct, not mediated by a chain.

- **It is not encrypted storage.** Objects are signed, not encrypted. Anyone who obtains an object can read it. End-to-end encryption is an application-layer concern (as demonstrated by the chat application).

- **It is not a general-purpose database.** The object store is append-only and content-addressed. It is optimized for immutable data with mutable pointers, not for relational queries or transactions.

- **It is not a replacement for the internet.** It is a local-first data layer with peer-to-peer capabilities. It complements the existing web; it does not replace it.

## Who is Canopee For?

Canopee is for developers building desktop applications that need:

- **Local-first data storage** with the ability to sync peer-to-peer
- **A portable identity** that works across applications
- **Decentralized app distribution** — publish a static site and serve it from any peer
- **Encrypted communication** — build chat, collaboration, or messaging features without a server
- **Data ownership** — users keep their data on their own machines

## The Workspace

Canopee is organized as a Rust workspace with eleven crates:

| Crate | Purpose |
|-------|---------|
| `canopee-identity` | Ed25519 keypair management, X25519 DH derivation |
| `canopee-config` | Path resolution, split-root model |
| `canopee-storage` | Object store, content-addressed file persistence |
| `canopee-protocol` | Wire protocol types (NodeCommand/NodeResponse) |
| `canopee-network` | libp2p swarm, discovery, DHT, pub/sub |
| `canopee-runtime` | Ties identity + storage + network together |
| `canopee-node` | Unix socket daemon, command handling |
| `canopee-sdk` | Client library for applications |
| `canopee-cli` | Command-line interface |
| `canopee-gateway` | WebSocket bridge for browsers |
| `canopee-e2e` | End-to-end integration tests |

Each crate is designed with clear boundaries. The layering is roughly: identity → storage → network → runtime → node → sdk → gateway → cli, with `canopee-config` (path resolution) and `canopee-protocol` (wire types) as shared crates consumed by the layers that need them. `canopee-e2e` contains no library code — it drives the compiled binaries as external processes.

## What's Next

The following chapters dive deep into each layer of the system. We'll start with the object model and work our way up through storage, networking, the runtime, the daemon, the SDK, and finally to building applications on top of Canopee.
