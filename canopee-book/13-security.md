# Chapter 13: Security Model

## Trust Boundaries

Canopee's security model is built on a clear set of trust boundaries:

1. **The node process**: fully trusted. Holds the private key, signs everything, enforces all invariants.
2. **The local machine**: trusted. The Unix socket is only accessible to local processes.
3. **The network**: untrusted. All objects and records from peers are verified before acceptance.
4. **The application**: semi-trusted. Can request operations but cannot forge signatures.

## What Is Signed

Everything in Canopee is signed by the owner's Ed25519 key:

- **Objects**: `signature = ed25519_sign(private_key, bincode(payload))`
- **Records**: signed envelopes containing the record data
- **AppPointers**: signed references to app manifests
- **Usernames**: signed claims

The signature covers all mutable fields. For *records*, verification checks both the cryptographic signature and that the public key derives the claimed identity. For *objects*, verification checks the content hash and the signature under the embedded public key, but does **not** bind the key to the claimed `owner` field — see the note in the Verification Pipeline below.

## What Is NOT Signed

- **Network addresses**: peer addresses are not authenticated
- **DHT records**: DHT providers are not verified beyond signature checking
- **Metadata**: timestamps (`created_at`) are self-reported and not verified

## What Is Encrypted

**Nothing in the storage layer is encrypted.** Objects are stored in plaintext on disk. On the network, objects are signed but not end-to-end encrypted at the application layer — but the libp2p transport itself *is* encrypted (see below).

End-to-end encryption exists as an application-layer pattern (demonstrated by the chat application), but it is not built into the storage or transport layers.

### At-Rest Encryption

Only the identity key can be encrypted at rest (via `CANOPEE_IDENTITY_PASS`). Object data, records, and cached data are always plaintext on disk.

### In-Transit Encryption

The libp2p transport is encrypted with **Noise** on both TCP and relay connections, so all wire traffic is encrypted and gossipsub messages are signed. This is transport-level encryption between directly-connected peers — it is not *end-to-end* between users across relays, and it is not certificate-authenticated (a relay sees the plaintext it forwards). TLS with a PKI is not used; the gap the roadmap refers to is the absence of certificate-authenticated, end-to-end transport.

## Signed ≠ Private

This is the most important security distinction in Canopee:

- **Signed** means: the object is verifiably authentic and unmodified
- **Private** means: only authorized parties can read it

Canopee provides the former, not the latter. A signed object is readable by anyone who obtains it. Sharing is controlled by the `HomeIndex` gate, not by encryption.

If you need confidentiality, use application-level E2E encryption (as the chat app does with X25519 + ChaCha20-Poly1305).

## The Verification Pipeline

Every object goes through verification at multiple points:

### On Write (Local)

```
put_verified(object)
  → check: id == sha256(bincode(payload))
  → check: ed25519_verify(public_key, signature, bincode(payload))
  → reject if either fails
```

### On Read (Local)

```
get_verified(id)
  → read file from disk
  → check: id == sha256(bincode(payload))
  → check: ed25519_verify(public_key, signature, bincode(payload))
  → reject if either fails
```

### On Network Fetch

```
fetch_object(peer, id)
  → receive ExportBundle from peer
  → (verification happens on import / get_verified, not in fetch itself)
```

Important nuances:

- `Runtime::fetch_object` imports (which verifies) before returning, so the embedded-app path is safe.
- The node `FetchObject` command, the SDK `fetch_object`, and the gateway `fetchObject` return the raw, **unverified** bundle to the caller. The gateway hands the unverified object straight to the browser. Verification happens in `import`/`get_verified`, not at fetch time.
- `Object::verify` checks `id == sha256(bincode(payload))` and the signature under the embedded public key — it does **not** check that the embedded public key derives the claimed `owner`. That binding exists only for `AppPointerRecord`.

### On Record Resolution

```
resolve_pointer(owner, name)
  → fetch record from DHT or local cache
  → check: ed25519_verify(signature)
  → check: public_key derives claimed owner
  → reject if either fails
```

There is no "trusted source" shortcut for *stored* objects — anything imported via `put_verified`/`import` is verified. The caveat is the fetch path: raw bundles returned over the wire by `FetchObject` are not verified until the caller imports them (and the gateway path never verifies). Additionally, object verification does not bind the signing key to the claimed `owner` field — only records do that.

## The Key Security Property: Signing in the Node

Clients never hold the private key. The SDK sends raw data; the node signs it. This means:

1. **No client-side key exposure**: the private key never leaves the node process
2. **No forged signatures**: no intercepted message can be replayed as a new version
3. **No version manipulation**: version numbers are incremented by the node, not the client
4. **No repudiation**: the node signs with its key, and the signature is verifiable by anyone

This is why the protocol has `PublishPointer` (takes raw data, node signs) rather than `PublishSignedPointer` (takes pre-signed data, node trusts it).

## Threat Model

### What Canopee Protects Against

| Threat | Protection |
|--------|------------|
| Object tampering | Content addressing + signature verification |
| Identity spoofing | Public key → PeerId derivation verification |
| Record forgery | Signed records (signature + owner binding verified) |
| Unauthorized access | HomeIndex sharing gate |
| Man-in-the-middle | Signature verification + Noise-encrypted transport |
| Replay attacks | ⚠️ Not currently prevented (see note) |

### What Canopee Does NOT Protect Against

| Threat | Mitigation |
|--------|------------|
| Eavesdropping | Application-level E2E encryption |
| Key theft | Physical security, encrypted at rest |
| Sybil attacks | No built-in rate limiting or reputation |
| DHT poisoning | Signature verification (bad records are rejected) |
| Denial of service | No built-in rate limiting |
| Metadata analysis | No mixnet or onion routing |

## The Gateway Security Model

The gateway adds a layer between browsers and the node:

- **Loopback-only**: no external connections
- **Session token**: prevents CSRF-like attacks
- **Origin enforcement**: only loopback origins accepted
- **No Shutdown**: web pages cannot shut down the node

But the gateway also adds a surface: any process on the local machine can connect to the WebSocket. This is acceptable because any local process can also read the Unix socket directly.

## Security Properties Summary

| Property | Status |
|----------|--------|
| Object integrity | ✅ Content addressing + SHA-256 |
| Object authenticity | ✅ Ed25519 signatures |
| Record integrity | ✅ Signed envelopes |
| Record authenticity | ✅ Public key derivation check |
| Identity self-sovereignty | ✅ No central authority |
| Data confidentiality | ⚠️ Application-layer only |
| Transport encryption | ✅ Noise (peer-to-peer; not certificate-authenticated) |
| At-rest encryption | ⚠️ Identity key only |
| Forward secrecy | ❌ Not implemented |
| Revocation | ❌ Not possible (by design) |
