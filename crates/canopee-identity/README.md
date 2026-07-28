# canopee-identity

Ed25519 keypair management for a Canopee node. Every node has exactly one
identity, created on first run and reused forever after — it's both the
node's signing key (for content-addressed objects, see
[`canopee-storage`](../canopee-storage)) and its libp2p `PeerId` (for the
network layer, see [`canopee-network`](../canopee-network)).

## Why one keypair for both roles

Reusing the same Ed25519 keypair as both the storage-signing key and the
libp2p identity means a node's network address and its object-signing
identity are the same thing: `canopee://identity/<peer id>`. There's no
separate "who signed this" vs "who's serving this" — an object's owner and
the swarm peer that can prove ownership are one and the same.

## API

```rust
use canopee_identity::Identity;

// First run: generates a new Ed25519 keypair and persists it to `path`.
// Subsequent runs: loads the same keypair back from disk.
let identity = match Identity::load("~/.canopee/identity/identity.key").await {
    Ok(identity) => identity,
    Err(_) => Identity::create("~/.canopee/identity/identity.key").await?,
};

// canopee://identity/<base58 PeerId>
let id = identity.id();

// Sign / verify arbitrary bytes.
let signature = identity.sign(b"some payload")?;
assert!(identity.verify(b"some payload", &signature));

// Protobuf-encoded public key, e.g. for embedding in a signed object.
let public_key_bytes = identity.public_key_bytes();

// The full libp2p Keypair, e.g. for handing to a SwarmBuilder.
let keypair = identity.keypair();
```

| Type | Purpose |
|---|---|
| `Identity` | Holds the keypair and derived `IdentityId`; sign/verify/create/load |
| `IdentityId` | `canopee://identity/<PeerId>` — the node's public, stable address |

`Identity::create`/`Identity::load` are async only because they do file I/O
(`tokio::fs`); the crypto itself is synchronous.

## Design notes

- The private key is stored on disk as raw protobuf-encoded key bytes
  (`Keypair::to_protobuf_encoding` / `from_protobuf_encoding`) with no
  additional encryption. Treat the identity file like an SSH private key —
  filesystem permissions are the only protection today.
- `IdentityId` wraps a `String`, not the underlying `PeerId`/`Keypair`
  directly, so it can be freely serialized (`serde`) and passed around the
  wire protocol (see [`canopee-protocol`](../canopee-protocol)) without
  pulling libp2p types into every consumer.
- `Identity::keypair()` clones the keypair. This is intentional — it's how
  [`canopee-network`](../canopee-network) hands the same key material to a
  libp2p `SwarmBuilder` without `canopee-identity` depending on the network
  crate.

## Testing

```bash
cargo test -p canopee-identity
```
