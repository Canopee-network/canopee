# Security considerations

A standing reference for what Canopee's current design does and doesn't
protect against — collecting scattered mentions across the other docs into
one place, plus a few not written down anywhere else yet. This isn't a
tutorial; it's a map of the trust boundaries so you know what you're
relying on (and what you're not) before building on top of any of it.

## What's already handled

- **Every `Object` is signed and content-addressed.** `ObjectId` is
  `sha256` of the payload ([`canopee-storage/src/object_id.rs`](../crates/canopee-storage/src/object_id.rs)),
  and `Verify::verify` checks both the id matches its content *and* the
  signature matches the embedded public key, on every read and write
  through `Storage` — not just at ingestion. Tampering with an object
  anywhere along its path (disk, network transfer, a malicious cache node)
  is detectable, not just at the source.
- **`AppPointerRecord::verify`** checks the signature *and* that the
  embedded public key actually derives the claimed `owner` IdentityId
  ([`canopee-storage/src/pointer.rs`](../crates/canopee-storage/src/pointer.rs)) —
  a peer can't serve you a pointer claiming someone else's name, even
  though pointers are mutable DHT records with no other access control
  (see [Namespace squatting](#namespace-squatting-on-app-pointers) below
  for what this *doesn't* cover).
- **An embedded node (Tauri-style) shrinks the local trust boundary.** See
  [Local trust boundary](#local-trust-boundary-the-nodes-socket) — this is
  actually a security argument in favor of the approach in
  [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md).

## No content or message encryption

Signed is not the same as private. Nothing in the current design encrypts
data for a specific recipient:

- **Objects are stored and transmitted in the clear**, verifiable by
  anyone who has them, but also *readable* by anyone who has them — any
  peer serving a `FetchObject` request, or a node that's cached a copy per
  [`p2p-app-caching-tutorial.md`](p2p-app-caching-tutorial.md), can see the
  full plaintext of anything they store.
- **Gossipsub messages are signed but not encrypted** — a relay you route
  through, or any peer subscribed to the same topic (deliberately or by
  guessing/knowing the topic string), can read message contents.
  [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md#step-6-end-to-end-message-encryption)
  now works through building this for a 1:1 chat, on top of the X25519
  key-agreement primitive in `canopee-identity`
  (`Identity::dh_public_key`/`agree` — see
  [`end-to-end-encryption.md`](end-to-end-encryption.md)). That tutorial
  step is optional/unbuilt-by-default, not something every app gets for
  free: don't market a chat app as private or secure unless it actually
  implements that step (or forward secrecy on top of it — the tutorial's
  stretch goal, still unbuilt anywhere in this codebase).
- **`AppPointerRecord`'s `manifest` field is plaintext** in the DHT record
  — anyone resolving `(owner, name)` sees exactly which manifest id is
  current, which is fine for a public app but means there's no private
  "publish under a name only specific people can resolve" mode.

## Transport security

- **`canopee open`'s HTTP server has no TLS.** `serve()` in
  [`crates/canopee-cli/src/app.rs`](../crates/canopee-cli/src/app.rs) is
  plain HTTP on `127.0.0.1` — fine for local viewing, a real gap the
  moment it's exposed further (see
  [`roadmap-hosting-replacement.md`](roadmap-hosting-replacement.md)'s
  HTTPS section for why this isn't a small fix — Canopee's identity-based
  addressing doesn't fit the standard DNS/CA certificate model).
- **The libp2p transport layer itself uses Noise** (see
  `noise::Config::new` in
  [`crates/canopee-network/src/manager.rs`](../crates/canopee-network/src/manager.rs)),
  so peer-to-peer connections are encrypted in transit between directly
  connected nodes. This is a different guarantee than *end-to-end*
  encryption — a relay in the middle of a relayed connection (see
  [`networking-for-beginners.md`](networking-for-beginners.md)) terminates
  its own Noise sessions on each side, so the relay itself can see
  whatever passes through it, even though eavesdroppers outside the
  connection can't.

## DHT and bootstrap trust

- **A fresh node has no way to cross-check a bootstrap address before
  trusting it.** [`bootstrap-nodes-tutorial.md`](bootstrap-nodes-tutorial.md#step-4-stretch-community-maintained-lists-instead-of-a-hardcoded-one)
  covers this directly: whoever controls the bootstrap list (hardcoded,
  DNS-based, or community-submitted) can steer new nodes toward
  attacker-controlled relays before those nodes have any other peer to
  compare against. The tutorial's recommendation — start with a small,
  curated hardcoded/DNS list, and require any community-contributed
  addresses to be signed by an allowlisted publisher rather than open
  submission — is the mitigation, not a solved problem.
- **`find_providers`/`get_record` results aren't otherwise authenticated
  beyond what the returned data itself proves.** A malicious DHT peer can
  answer a `find_providers` query with a bogus or unresponsive peer id;
  the caller only finds out once it actually tries `fetch_object` against
  that peer and gets nothing back or garbage that fails `Verify::verify`.
  This is self-correcting for *objects* (a forged object simply fails
  signature verification) but not for the *availability* claim itself — a
  hostile or overloaded provider can waste a fetcher's time before the
  fetcher falls back to another candidate (see
  [`p2p-app-caching-tutorial.md`](p2p-app-caching-tutorial.md#step-2-dont-just-take-the-first-provider--try-them-all)'s
  Step 2 for why trying multiple providers matters, partly for exactly
  this reason).

## Cache poisoning and availability attacks

Once fetch-and-reannounce caching exists
([`p2p-app-caching-tutorial.md`](p2p-app-caching-tutorial.md)), any node
that's fetched an object can announce itself as a provider of it. This
has a built-in limit and a gap:

- **Can't forge content** — a cache node claiming to provide an object it
  doesn't actually have, or serving tampered content, is caught by the
  same `Verify::verify` check every other path already runs. Content
  integrity survives caching without any new code.
- **Can refuse or degrade service with no consequence.** There's no
  reputation or accountability mechanism — a cache node can announce
  itself as a provider and then simply not respond, respond slowly, or
  serve stale/evicted content, and nothing currently downgrades or
  excludes it from future `find_providers` results. This is a real
  availability (not integrity) attack surface once caching ships, and
  isn't solved by anything designed so far — worth treating as a known
  gap if caching moves beyond a single trusted deployment.

## Namespace squatting on app pointers

`AppPointerRecord` keys are derived purely from `(owner, name)` — there's
no registry preventing two different names from colliding, but there's
also no protection *within* a single owner's own namespace: nothing stops
you from publishing under a name you don't have any established claim to
beyond "I got there first, using my own identity." This is different from
DNS-style squatting (you can't take someone *else's* name — the key is
derived from *your own* `owner` id, so `alice`'s pointer for
`"portfolio"` and `bob`'s pointer for `"portfolio"` are different DHT
keys entirely) but means there's no human-readable global namespace at
all — `(owner, name)` is only ever meaningful to someone who already
knows which owner they're looking for.

## Key management

- **No revocation.** If an identity's private key is compromised, there is
  no mechanism to invalidate objects or pointers already signed with it —
  Ed25519 key rotation isn't designed for anywhere in
  [`canopee-identity`](../crates/canopee-identity/README.md). A
  compromised key can keep publishing indefinitely under the victim's
  established identity until the victim notices and tells their contacts
  to stop trusting that `IdentityId` — a purely social, out-of-band
  mitigation, not a protocol-level one. This also means there's no
  device-scoped way to use one identity from multiple devices safely —
  see [`multi-device-identity.md`](multi-device-identity.md), whose only
  option today is copying the same private key to every device, with the
  same lack of revocation and no per-device trust separation at all.
- **The private key file is stored unencrypted on disk.**
  `Identity::create`/`load` write/read raw protobuf-encoded key bytes with
  no passphrase or OS-keychain integration
  ([`crates/canopee-identity/src/identity.rs`](../crates/canopee-identity/src/identity.rs)).
  The `canopee-identity` README already says this outright: "treat the
  identity file like an SSH private key — filesystem permissions are the
  only protection today." Worth restating here because it's the single
  point of failure behind every other guarantee in this document — every
  signature, every pointer, every object's provenance traces back to
  whether this one file stayed private.

## Local trust boundary: the node's socket

`canopee-node` listens on a Unix domain socket
([`crates/canopee-config/src/lib.rs`](../crates/canopee-config/src/lib.rs)'s
`node_socket_path`) with no authentication beyond OS filesystem
permissions on the socket file itself. Anything on the same machine that
can open that socket has full control of the identity behind it — sign
objects, publish to any topic, dial arbitrary peers, everything
`NodeCommand` exposes.

This is the actual argument, stated precisely, for the embedded-runtime
approach in [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md):
linking `canopee-runtime` directly inside your own process means there's
no separate socket at all — the trust boundary collapses to "anything
running inside your own app's process," which is normally a smaller,
better-understood attack surface than "anything on the machine that can
reach a well-known socket path." It doesn't eliminate the underlying
risk (a compromised app process is just as bad as a compromised socket
client), but it removes an entire *other* process from the equation.

## Summary table

| Concern | Status | Where it's covered |
|---|---|---|
| Object tampering/impersonation | Handled — signed, content-addressed, verified on every access | `canopee-storage` README |
| App pointer impersonation | Handled — signature + owner-derivation check | `AppPointerRecord::verify`, `app-manifests.md` |
| Content/message privacy | **Not handled by default** — signed ≠ encrypted; key-agreement primitive exists, and a tutorial builds on it, but no app gets this for free | This doc, [`end-to-end-encryption.md`](end-to-end-encryption.md), `tauri-chat-app-tutorial.md` Step 6 |
| `open`'s HTTP server transport | **Not handled** — plain HTTP | `roadmap-hosting-replacement.md` |
| Direct peer-to-peer transport | Handled — Noise-encrypted | `canopee-network` (relayed hops excepted) |
| Bootstrap/DHT poisoning | Partial — mitigation designed, not built | `bootstrap-nodes-tutorial.md` Step 4 |
| Cache node misbehavior (availability) | **Not handled** — no reputation system | This doc |
| Global human-readable namespace | Not applicable — `(owner, name)` isn't global by design | This doc |
| Key compromise / revocation | **Not handled** | This doc |
| Private key at rest | **Not handled** — plaintext file, permissions only | `canopee-identity` README |
| Local socket authorization | Limited to OS file permissions; embedding avoids the issue | This doc, `tauri-chat-app-tutorial.md` |

Treat "Not handled" rows as open work, not as bugs to file — none of them
have an assigned owner or a tutorial yet. If you pick one up, the existing
tutorials' style (step-by-step, verify-before-moving-on, name the real
design tradeoffs) is the pattern to follow.
