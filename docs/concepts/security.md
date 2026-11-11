# Security

Canopee's model is: **you own your keys, your data, and your decisions.** This
page states what the platform handles, what it deliberately does not, and where
an application's responsibilities start.

## What Canopee handles

**Identity and signatures.** Objects, pointers, records, and capability grants
are signed by Ed25519 identity keys. Any peer can verify authenticity and
integrity offline: object hashes for content, signatures for everything,
content-derived ids for capabilities.

**Content-addressing.** `id = hash(type || data)` means stored or fetched bytes
are checked against their id before use. Corruption and tampering in transit are
detected.

**Recipient-scoped encryption.** Objects written through `put` /
`put_object` are encrypted by default with the envelope in
`canopee-identity/src/envelope.rs`:

- one random XChaCha20-Poly1305 content key encrypts the payload;
- the content key is wrapped for the owner's X25519 key and for each explicit
  recipient using ephemeral-static X25519 + HKDF-SHA256;
- the owner is always a recipient, so storing an object cannot make it
  unreadable to its owner;
- because pairing copies the identity key, every device carrying that identity
  derives the same DH key and can read the owner's objects.

The ciphertext, not the plaintext, is what is content-addressed, stored,
replicated, and served. A recipient share creates a separate encrypted copy
whose id differs from the owner's private original.

**Identity protection at rest (optional).** If `CANOPEE_IDENTITY_PASS` is set,
the identity key file is stored password-encrypted with Argon2id +
XChaCha20-Poly1305 and unlocked on node start. Without it, the key file is
plaintext and protected only by filesystem permissions. Identity export is
always passphrase-encrypted.

**Pairing secrecy.** Pairing credentials are exchanged over an ECDH/X25519
session-key-secured channel (`pairing_session_key`,
`encrypt_pairing_payload` in `crates/canopee-runtime/src/pairing.rs`), so the
transferred identity key is never transmitted in plaintext. Pairing is currently
LAN-first; relay-only pairing is being hardened.

**Scoped sharing.** Nothing is shared by default; explicit `share` marks an
object servable and announces it as a DHT provider. `unshare` withdraws both.
Independently, capability grants gate read/write/publish permissions per
subject-resource ([Capabilities](capabilities.md)).

**Local API isolation.** The node speaks `canopee-protocol` over a Unix socket
in the user's own data directory, not a TCP port anyone on the LAN can reach.
The WebSocket gateway is loopback-only and session-token-gated.

## Deliberately unencrypted data

Some bytes must remain readable by peers or by a public edge. These exceptions
are explicit in the code:

- **App artifacts** written through `put_object_public` (manifests, entrypoints,
  assets). Apps are public output; an edge has no identity key and could not
  decrypt them.
- **Signed control records needed for discovery**: pointer-record objects,
  profiles, usernames, device lists, and capability indexes. These contain the
  public keys and routing information peers need to find and verify an owner.
- **Usernames and registry entries** published as DHT records.

Contacts and the home index are encrypted to the owner's identity. Their
pointers and object ids may still be public, but the contents are not.

## Metadata that remains visible

Encryption protects object payloads, not the fact that objects exist. A relay,
peer, or provider can observe:

- object ids, object types, creation timestamps, and plaintext sizes
  (`metadata.size` is kept in plaintext so UIs can show real file sizes);
- owner identity ids and, for public records, profile display names, usernames,
  DH public keys, device names, and device peer ids;
- DHT keys and signed pointer records (for example, a share name and the object
  id it points to);
- provider announcements, peer ids, connection timing, and approximate traffic
  volume.

If metadata privacy matters, encrypt and pad at the application layer.

## What Canopee does not guarantee

**Forward secrecy for stored objects.** Object envelopes use ephemeral-static
X25519 against long-lived identity keys. Compromise of an identity key exposes
objects encrypted to it; there is no per-message ratchet in the object store.

**Revocation of already-fetched data.** `unshare` stops future serving and
provider discovery. It cannot erase copies another peer already downloaded, and
a recipient can still decrypt a copy they hold. Folder-sync deletes propagate
as tombstones so a stale device cannot resurrect them; tombstones are retained
for 30 days (wall clock) and do not revoke copies already on disk. Independent
device revocation still requires the delegated-key migration described in
[Canopee Sync v1](../reference/sync-v1.md#identity-device-and-key-model).

**Serving quotas, rate limiting, and abuse protection.** There is no built-in
rate limiting, bandwidth fairness, or spam matrix for fetched content or
gossipped messages. These are per-node follow-up work.

**Anonymity or unlinkability.** Peer ids, provider records, and DHT lookups are
observable. Canopee is not an anonymity network.

**Semantic search, moderation, or global metadata indexes.** The DHT stores
provider and registry records, not content. There is no global index of who has
what.

**A professional cryptographic audit.** The protocol and implementation have not
yet had an independent security review. Treat this as a reference
implementation, not a production security boundary.

**Perfect forward secrecy on relayed connections.** Connections are
Noise-encrypted hop by hop. DCUtR-upgraded direct connections are preferable to
relayed ones; relays remain a fallback for unreachable nodes and cannot decrypt
object payloads.

## Application-layer responsibilities

- Use `put` / `put_object` for sensitive data. Do not use `put_object_public`
  unless the bytes are intentionally public.
- Treat filenames, sizes, timestamps, and sharing relationships as visible
  metadata; encrypt or pad before storing if that is not acceptable.
- Store and validate received capabilities before exercising them.
- Use the SDK's verified-fetch path for network-sourced objects; never trust raw
  bytes without id verification.
- Never log identity keys, pairing payloads, plaintext object contents, or
  passphrases.

## Threat model summary

| Threat | Status |
|---|---|
| Attacker forges an object or record in your name | prevented by signatures |
| Attacker replays an old valid record | visible via timestamps; republish to supersede |
| Attacker serves wrong bytes for an id | detected by content-hash mismatch |
| Attacker reads an unshared object | not served; protected by availability, not encryption of copies already obtained |
| Attacker fetches a shared encrypted object | receives ciphertext; cannot read without a recipient key |
| Attacker reads a public app artifact | possible by design |
| Attacker extracts the identity key from the network | prevented: not transmitted; pairing payload is session-encrypted |
| Attacker steals a device key | impersonates that device until it is removed; identity key is separate |
| Attacker steals the identity key | can impersonate and decrypt everything for that identity |
| Attacker observes relayed traffic | libp2p Noise protects the connection; payloads are also encrypted |
| Attacker DoSes your node with fetches | currently unmitigated; no quotas yet |
| Attacker learns filenames/sizes/sharing graph | possible from metadata and public records |
