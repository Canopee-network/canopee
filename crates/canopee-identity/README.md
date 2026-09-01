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

// X25519 key agreement, for end-to-end encryption above this crate (see
// "Key agreement (X25519)" below). Publish `dh_public_key()` however your
// app discovers peers; it is not resolved by `IdentityId` today.
let my_dh_public_key = identity.dh_public_key();
let shared_secret = identity.agree(&their_dh_public_key); // run through a KDF before using as a cipher key

// Encrypted at rest (see "Encrypted identity at rest" below).
let (identity, encrypted_at_rest) = Identity::load_encrypted(
    "~/.canopee/identity/identity.key",
    "my-passphrase",
).await?;
```

| Type | Purpose |
|---|---|
| `Identity` | Holds the keypair and derived `IdentityId`; sign/verify/create/load/encrypted-at-rest |
| `IdentityId` | `canopee://identity/<PeerId>` — the node's public, stable address |

`IdentityId::new(id)` builds one from a raw string — e.g. one another user
shared out of band (an identity string printed by their `canopee identity`)
— rather than deriving it from a keypair you hold. It does no validation of
the string's shape; malformed input just fails to resolve to anything later
(e.g. an app pointer lookup under a bogus owner simply finds nothing).

`Identity::create`/`Identity::load` are async only because they do file I/O
(`tokio::fs`); the crypto itself is synchronous. `create_encrypted`/`load_encrypted`
share the same property.

## Encrypted identity at rest

The `create_encrypted`/`load_encrypted` methods encrypt the on-disk key
with Argon2id (memory-hard password hashing) and XChaCha20-Poly1305 (AEAD),
so an attacker with access to the filesystem alone cannot recover the
signing key without the passphrase. Set the `CANOPEE_IDENTITY_PASS`
environment variable when starting `canopee-node` to enable encrypted-at-rest
for that node.

```text
# Create an encrypted identity on first run:
CANOPEE_IDENTITY_PASS="my-passphrase" cargo run -p canopee-node

# Subsequent starts with the same passphrase load correctly:
CANOPEE_IDENTITY_PASS="my-passphrase" cargo run -p canopee-node

# A wrong passphrase fails immediately with a clear error.
```

Important:
- `create_encrypted`/`load_encrypted` are opt-in — `create`/`load` remain
  unchanged and write plaintext protobuf. Existing deployments are unaffected
  until they explicitly opt in.
- Setting `CANOPEE_IDENTITY_PASS` when an existing plaintext key file is
  present does **not** retroactively encrypt it; the runtime logs a warning.
  To encrypt, delete the key file and restart with the env var set.
- The passphrase lives only in the process environment for the duration of
  the run; there is no disk-stored recovery, and losing the passphrase
  means losing the identity permanently.
- The on-disk format is a versioned envelope (`canopee-v1-ek`, Argon2id v19
  with parameters stored in the envelope, 192-bit nonce) — loadable by any
  canopee version that ships the same envelope reader.

## Key agreement (X25519)

Signing proves who sent something; it doesn't make it private (see
[`security-considerations.md`](../../docs/security-considerations.md)).
`dh_public_key()`/`agree()` add the other half — a way for two identities to
derive a shared secret, as the basis for end-to-end encryption built by
whatever's layered on top (e.g. a chat app's session/ratchet logic). This
crate deliberately stops at the raw shared secret:

- `dh_public_key()` returns this identity's X25519 public key, derived (not
  separately generated) from the same Ed25519 signing key via
  `Keypair::derive_secret`, domain-separated with a fixed `canopee/dh/...`
  string. Same signing key in ⇒ same DH keypair out, every time — nothing
  new to persist, lose, or get out of sync with the identity file.
- `agree(their_dh_public_key)` runs Diffie-Hellman against another
  identity's DH public key and returns a raw `[u8; 32]`. It is *not* a
  cipher key — run it through a KDF, mix in whatever nonce/session material
  your protocol needs, and manage ratcheting/forward-secrecy yourself.
  Nothing here tracks conversation or session state.
- There's no built-in way to discover someone's `dh_public_key()` from just
  their `IdentityId` — publishing/fetching it (as a signed object, a DHT
  record, or anything else) is left to the caller, the same way this crate
  doesn't prescribe how `public_key_bytes()` gets distributed either.

## Design notes

- **Plaintext mode** (the default): the private key is stored on disk as raw
  protobuf-encoded key bytes (`Keypair::to_protobuf_encoding` /
  `from_protobuf_encoding`) with no additional encryption — filesystem
  permissions are the only protection. **Encrypted mode**
  (`create_encrypted`/`load_encrypted`, or when `CANOPEE_IDENTITY_PASS` is
  set in `canopee-runtime`): the protobuf is wrapped in a versioned
  envelope encrypted at rest with Argon2id + XChaCha20-Poly1305, as
  described in "Encrypted identity at rest" above. Both modes load the
  same `Identity` type; the only difference is what ends up on disk.
- `IdentityId` wraps a `String`, not the underlying `PeerId`/`Keypair`
  directly, so it can be freely serialized (`serde`) and passed around the
  wire protocol (see [`canopee-protocol`](../canopee-protocol)) without
  pulling libp2p types into every consumer.
- `Identity::keypair()` clones the keypair. This is intentional — it's how
  [`canopee-network`](../canopee-network) hands the same key material to a
  libp2p `SwarmBuilder` without `canopee-identity` depending on the network
  crate.
- **Never reuse the signing key and the DH secret across purposes**, even
  though both are derived from the same Ed25519 seed. `derive_secret`'s
  domain separation (`canopee/dh/x25519/v1`) exists specifically so the DH
  secret is cryptographically independent of the raw signing key — sign
  with `sign()`, agree with `agree()`, never the other's underlying key
  material. This is a real, well-known class of cross-protocol key-reuse
  bugs, not a style preference.

## Testing

```bash
cargo test -p canopee-identity
```
