# Security

Canopee's model is: **you own your keys, your data, and your decisions.** This
page states clearly what the platform handles, what it deliberately does not,
and where your application's responsibilities start.

## What Canopee handles

**Identity & signatures.** Objects, pointers, records, and capability grants
are signed by identity keys (Ed25519). Any peer can verify authenticity and
integrity offline — hash recomputation for objects, signature verification for
everything, content-derived ids for capabilities. This is the core guarantee.

**Content-addressing.** `id = hash(type || data)` means stored or fetched
bytes are always checked against their id before use. Corruption and
tampering-in-transit are detected.

**At-rest identity encryption (optional).** If `CANOPEE_IDENTITY_PASS` is set,
the identity key file is stored password-encrypted and unlocked on node start.

**Pairing secrecy.** Pairing credentials are exchanged over an ECDH/X25519
session-key-secured channel (`pairing_session_key`, `encrypt_pairing_payload`
in `crates/canopee-runtime/src/pairing.rs`), so the transferred identity key
is never transmitted in plaintext.

**Scoped sharing.** Nothing is shared by default; explicit `share` upserts
the home index, publishes a pointer, and announces a provider. `unshare`
withdraws both. Independently, capability grants gate *read/write/publish*
permissions per subject-resource ([Capabilities](capabilities.md)).

**Local API isolation.** The node speaks `canopee-protocol` over a Unix socket
in the user's own data directory — not a TCP port anyone on the LAN can reach.
The WebSocket gateway is loopback-only and session-token-gated.

## What Canopee does not (yet) guarantee

**Encryption in transit / at rest for content (default).** Object *bytes* are
not encrypted by default. Anyone who can fetch an object can read it. If your
data must be private:

- encrypt the payload at the app layer before `put`, or
- don't share it (nothing is published without a share), or
- grant access via capabilities (availability control), and
- use E2E channels for conversations. An X25519 DH public key is published in
  the profile specifically so apps can derive conversation keys without an
  extra round trip (see `docs/archive/legacy-docs/end-to-end-encryption.md`
  for the design notes).

**Serving quotas / rate limiting / abuse protection.** There is no built-in
rate limiting, bandwidth fairness, or spam matrix for fetched content or
gossipped messages. These are per-node config and current work.

**Semantic search, moderation, or global metadata indexes.** The DHT stores
provider and registry records, not your content or its semantics. There is no
global index that knows "what everyone has" — that's intentional.

**Perfect forward secrecy on relayed connections.** A DCUtR-upgraded direct
connection is preferable to a relayed one; relays are a fallback for
unreachable nodes. The connection itself is noise-secured as-is from libp2p.

**Social-proof trust.** Capabilities and records prove *who* you are, not
*whether* to trust you. Reputation and trust are application-layer.

## App-layer responsibilities

- Encrypt anything sensitive before publishing (see above).
- Store any capability you *receive* somewhere retrievable and validate it
  before exercising it.
- Treat `public_key`/`signature` fields as opaque trusted elements (they are
  excluded from signing bytes by design — see the capability implementation).
- Use the SDK's verified-fetch path (`get`-style) for all network-sourced
  objects; never trust raw bytes without id verification.

## Threat model summary

| Threat | Status |
|---|---|
| Attacker forges an object/record in your name | prevented — signatures |
| Attacker replays an old valid record | visible — timestamped; revocation by re-publish |
| Attacker serves wrong bytes for an id | detected — content hash mismatch |
| Attacker reads your unshared data | prevented — nothing public without a share |
| Attacker extracts identity key from network | prevented — not transmitted; session-secured when pairing |
| Attacker steals device key | contained — rotate device, not identity (see [Identity](identity.md)) |
| Attacker DoSes your node with fetches | currently unmitigated — no quotas yet |
| Attacker eavesdrops relayed traffic | libp2p-noise provides connection-level encryption |