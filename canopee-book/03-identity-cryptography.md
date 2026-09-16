# Chapter 3: Identity and Cryptography

## The Identity Model

In Canopee, identity is a cryptographic keypair. There is no username/password, no OAuth flow, no account creation. Your identity is a file on disk — an Ed25519 keypair — and that file *is* you.

```rust
struct Identity {
    signing_key: Keypair,       // Ed25519 — signs objects
    dh_secret: DhSecret,        // X25519 secret, deterministically derived
    pub identity_id: IdentityId, // canopee://identity/<peer-id>
}
```

The account public key derives a `PeerId` (libp2p's identity primitive), and the full identity URI is:

```
canopee://identity/<base58-encoded-peer-id>
```

This URI is your identity across all Canopee applications. It never changes. It cannot be revoked, because there is no authority to revoke it. It is self-sovereign in the truest sense: you generate it, you hold it, you own it.

## Account Key vs. Device Key

A Canopee node holds two keypairs, and it's worth keeping them straight:

1. **The account key (`Identity`)** — the *person*. Ed25519; signs objects and derives the X25519 DH secret. Its public key derives the `IdentityId` above — the account URI that never changes. This key can be shared across a user's devices.
2. **The device key (`DeviceKey`)** — the *machine*. A separate Ed25519 keypair minted once per device (see [`crates/canopee-identity/src/device.rs`](../crates/canopee-identity/src/device.rs)), never transferred. Its public key is the libp2p `PeerId` the swarm dials and is dialed with.

Splitting the *network* identity from the account was deliberate: several devices of one identity can now be online at once, each with its own `PeerId`, without colliding on the network — while ownership of objects stays tied to the one shared account key. "Who signed this" is the account URI; "who is serving this" is the device's `PeerId`. The two are linked by a
`device:<peer-id> → identity` registry record and the owner's
`(owner, "devices")` list (see the multi-device section below and Chapter
8's command table).

## Key Generation and Storage

On first run, Canopee generates an Ed25519 keypair and stores it at:

```
~/.canopee/identity/identity.key
```

The key is stored in protobuf encoding by default. The `create_if_absent` method ensures that:

- If no key exists, one is generated
- If a key exists, it is loaded
- The operation is race-safe for shared user roots (two applications starting simultaneously won't create two keys)

### Encrypted at Rest

For sensitive deployments, the identity key can be encrypted at rest:

```bash
export CANOPEE_IDENTITY_PASS="my-secret-passphrase"
```

When this environment variable is set, new identity keys are encrypted using:

1. **Key derivation**: Argon2id v19 with parameters stored in the envelope
2. **Symmetric encryption**: XChaCha20-Poly1305
3. **Envelope format**: `canopee-v1-ek` magic + an 18-byte version tag (`argon2id/xchacha20`) + three u32 little-endian Argon2 parameters (memory, iterations, parallelism) + salt[16] + nonce[24] + ciphertext‖tag

The passphrase is required on every startup to decrypt the key. If the passphrase is wrong, you get an immediate, clear error. If you lose the passphrase, you lose the identity permanently — there is no recovery mechanism, by design.

**Important**: setting the environment variable over an existing plaintext key does *not* retroactively encrypt it. You must delete the key file and let it be regenerated under the passphrase.

## X25519 Diffie-Hellman Key Agreement

Beyond signing, Canopee identity supports key agreement for end-to-end encryption. From the same Ed25519 keypair, an X25519 public key is derived:

```rust
fn dh_public_key(&self) -> [u8; 32]
```

The derivation uses `Keypair::derive_secret` with the domain separation constant `canopee/dh/x25519/v1`. The DH secret is deterministically derived *from* the signing key — reloading the same keypair always reproduces the same DH keypair. Domain separation guarantees the reverse one-wayness: you cannot recover the signing key from the DH key, and the DH key is cryptographically independent of any other `derive_secret` output.

To establish a shared secret with another peer:

```rust
let shared_secret = identity.agree(their_dh_public_key);
// shared_secret must be run through a KDF (HKDF etc.) before use
```

The `agree` function returns a raw 32-byte shared secret. **This must be processed through a key derivation function** (such as HKDF-SHA256) before using it as an encryption key. The raw output of Diffie-Hellman has biases that make it unsuitable for direct use as a cipher key.

A peer's DH public key is discoverable from their `IdentityId`: it is published in their `Profile` record (under the well-known `(owner, "profile")` pointer) and carried in their `ContactList`. Resolving `(owner, "profile")` → `dh_public_key` lets any application derive an E2E conversation key without an extra lookup. Alternatively, the DH key can be shared directly out-of-band via a contact string (see Chapter 14).

### Domain Separation

The `derive_secret` call uses a domain-separated constant (`canopee/dh/x25519/v1`) to ensure that:

- The DH key cannot be used to recover or forge the signing key
- The DH key is cryptographically independent of any other `derive_secret` output
- Cross-protocol key-reuse attacks are impossible

This is a critical security property. Even though the DH key is derived from the signing key, the derivation is one-way and domain-separated, so the two keys are cryptographically independent in practice.

## The canopee:// URI Scheme

Canopee defines a URI scheme for referencing identities and records:

```
canopee://identity/<peer-id>
canopee://identity/<peer-id>/<record-name>
canopee://<alias>/<record-name>
```

Examples:
- `canopee://identity/12D3KooWABCD...` — an identity
- `canopee://identity/12D3KooWABCD.../profile` — a profile record
- `canopee://alice/profile` — a profile by username alias

The URI scheme is parsed by the CLI's `uri.rs` module and provides a human-readable way to reference peers, records, and aliases.

## Multi-Device Identity

One identity now lives across many devices through a **pairing** model, not by copying one key file around by hand:

- Each device keeps its own long-lived `DeviceKey` (its network `PeerId`), minted on first run and registered in the identity's `(owner, "devices")` record.
- The shared account key is transferred to a new device **once**, over the LAN: the new device runs `canopee pair` and shows a 12-character code; the device that already carries the identity approves with `canopee pair <qr-payload> --code <code>`. The source then encrypts the account key plus its signed user records (device list, profile, contacts) under a code-derived session key and delivers them to the new device (`/canopee/pairing/1.0.0`). The new device takes the identity over on its next restart; its old key is backed up first.
- Records stay in step afterward through a periodic DHT sync and an explicit `canopee sync [peer-id]` (refresh from one peer or from every registered device).

Honest limitations remain:

- **The account key is still one secret.** Pairing transfers it; a device can be dropped from the `devices` list, but there is no per-device key the account is split across, so a stolen copy can't be revoked independently without abandoning the account.
- **Pairing is not account recovery.** The 12-character code is a live, single-use approval — it derives the session key, not a recoverable passphrase. Lose the account key everywhere and the identity is gone.
- **No delegation** — there is no way to grant limited access to your identity.

For most use cases, this is exactly right. Your identity is your key. Guard it.

## Security Properties

### What Signatures Guarantee

- **Authorship**: only the holder of the private key could have created this object
- **Integrity**: the object has not been modified since it was signed
- **Non-repudiation**: the author cannot deny having signed the object

### What Signatures Do Not Guarantee

- **Privacy**: signed objects are readable by anyone who obtains them
- **Freshness**: a signature tells you who signed it, not when (beyond the `created_at` metadata, which is self-reported)
- **Authorization**: a signature proves identity, not permission. Sharing is controlled by the `HomeIndex`, not by signatures

### The Trust Model

Canopee's trust model is direct:

- You trust an object if its signature verifies under a public key you trust
- You trust a peer if it serves objects that verify correctly
- There is no certificate authority, no web of trust, no reputation system

This is intentional. Canopee provides the cryptographic primitives for verification; applications layer trust decisions on top.
