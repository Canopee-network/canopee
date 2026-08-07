# End-to-end encryption: what's built, what isn't

A standing reference for the X25519 key-agreement primitive in
[`canopee-identity`](../crates/canopee-identity) — what it actually gives
you, and the much larger set of things it deliberately does *not* do. Read
[`security-considerations.md`](security-considerations.md) first; this doc
is the detailed follow-up to that doc's "No content or message encryption"
section, now that part of the gap has a primitive to close it.

**The short version: canopee gives you a way for two identities to derive a
shared secret. It does not encrypt anything. No message sent through
canopee today is any more private than before this primitive existed,
until an app actually uses it.**

## What exists

`Identity` (in [`crates/canopee-identity/src/identity.rs`](../crates/canopee-identity/src/identity.rs))
exposes two methods alongside its existing Ed25519 sign/verify API:

```rust
let my_dh_public_key = identity.dh_public_key();       // [u8; 32], safe to share
let shared_secret = identity.agree(&their_dh_public_key); // [u8; 32], DH output
```

- **`dh_public_key()`** returns an X25519 public key, derived from the same
  Ed25519 signing key every identity already has — via libp2p's own
  `Keypair::derive_secret`, domain-separated with a fixed
  `canopee/dh/x25519/v1` string. There's no second key file: reloading the
  same identity always reproduces the same DH keypair, deterministically.
- **`agree(their_dh_public_key)`** performs X25519 Diffie-Hellman and
  returns the raw shared secret. Two identities that each call `agree`
  with the other's `dh_public_key()` arrive at the same 32 bytes —
  verified by `dh_agreement_is_symmetric` in
  [`crates/canopee-identity/src/identity.rs`](../crates/canopee-identity/src/identity.rs).

That's the entire feature. It answers exactly one question: *"given I know
another identity's DH public key, what secret can only the two of us
derive?"* — nothing about how to find that key, what to encrypt with the
secret, or how to keep a conversation private over time.

## Why this, and not a full encrypted-messaging protocol

This was a deliberate platform/application split, not a partial
implementation of something bigger:

- Ed25519 (signing) and X25519 (key agreement) are different curves for
  different purposes. Canopee's one-keypair-per-identity model
  ([`canopee-identity`](../crates/canopee-identity/README.md)) only had
  the signing half; apps that wanted encryption had no way to get a DH
  keypair without hand-rolling an Ed25519→X25519 conversion themselves —
  exactly the kind of "everyone reimplements it slightly wrong" gap a
  shared crate exists to close.
- Everything above raw key agreement — session/ratchet state, forward
  secrecy, group key distribution, message framing — is protocol logic
  specific to *what's being built* (a chat app's needs differ from a
  presence app's or a collab editor's), not something canopee's
  general-purpose object/pub-sub substrate should prescribe. Baking a
  specific messaging protocol into `canopee-identity` would be scope creep
  into every other kind of app on the network.

## What still has to be built above this, per feature

None of the following exist in `canopee-identity`/`canopee-storage`/
`canopee-network` themselves — they're app-layer, by design (see above).
[`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md#step-6-end-to-end-message-encryption)'s
Step 6 now walks through building the first three for a 1:1 chat; the
last two remain open even there.

- **Key discovery.** There is no way to look up an identity's
  `dh_public_key()` from just its `IdentityId`. Nothing publishes or
  resolves it — an app has to decide how (e.g. a signed object analogous
  to `AppManifest`, a DHT record analogous to `AppPointerRecord`, or
  something out-of-band) and build it. Built for a 1:1 chat in the
  tutorial's Step 6a, as a signed object a contact fetches directly.
- **Turning the shared secret into a cipher key.** `agree()`'s output is
  raw DH material, not a symmetric key — it must be passed through a KDF
  (e.g. HKDF) before use, and never used directly to encrypt. Built in
  Step 6b.
- **Actually encrypting anything.** Nothing in canopee itself encrypts
  before calling `network.publish` or storing an `Object`. Gossipsub
  messages remain signed-but-plaintext and objects remain readable by
  anyone who has them at the platform level, exactly as described in
  [`security-considerations.md`](security-considerations.md#no-content-or-message-encryption) —
  this primitive changes nothing about that by itself; an app has to wire
  encryption in front of those calls, as Step 6c does.
- **Forward secrecy / ratcheting.** A single static shared secret per
  identity pair, reused for every message (which is what Step 6b/6c
  produce), is exactly what Signal's Double Ratchet protocol exists to
  avoid: one compromised secret would expose every past and future
  message between that pair. Canopee provides no ratchet, no per-message
  key derivation, no session state at all, and the tutorial's Step 6
  doesn't add one either — it's called out there as stretch/follow-up
  work, not solved.
- **Group messaging.** Pairwise DH only ever produces a secret between
  two identities. Group chat key distribution (sender keys, pairwise
  fan-out, or otherwise) is a distinct, harder design problem, entirely
  unaddressed anywhere in this codebase, tutorial included.
- **Key rotation independent of identity rotation.** Because
  `dh_public_key()` is deterministically derived from the long-lived
  signing key, there's no way to rotate just the DH keypair without
  rotating the whole identity — and identity rotation itself isn't
  supported either (see "No revocation" in
  [`security-considerations.md`](security-considerations.md#key-management)).

## Where this leaves message privacy today

At the platform level, nothing has changed: messages published via
`network.publish` or stored as `Object`s are exactly as private as before
this primitive was added — not private at all, readable by any
subscriber, relay, or holder of the object, unless the app in front of
those calls encrypts first. `Identity::agree()` is a necessary building
block, not a fix by itself; the tutorial's Step 6 is the first place in
this codebase that actually closes the gap (for a 1:1 chat, without
forward secrecy). Don't describe a canopee-based app as end-to-end
encrypted on the strength of the primitive alone — only once it's actually
using something equivalent to Step 6, and even then, be explicit that
forward secrecy and groups aren't included.

## Design notes

- **Never reuse the signing key and the DH secret for each other's
  purpose**, even though both derive from the same Ed25519 seed.
  `derive_secret`'s domain separation exists specifically to keep them
  cryptographically independent — this is a real, well-documented class of
  cross-protocol key-reuse bug, not a style nitpick. See the design note
  in [`canopee-identity/README.md`](../crates/canopee-identity/README.md).
- `agree()` deliberately returns a raw `[u8; 32]` rather than a typed
  "shared secret" or "session key" — the rawest possible primitive, so
  nothing about it implies a KDF, nonce, or ratchet has already happened.

## Testing

```bash
cargo test -p canopee-identity
```

`dh_agreement_is_symmetric` and `dh_public_key_is_deterministic_across_load`
cover the primitive itself; there is nothing to test above it yet because
nothing above it has been built.
