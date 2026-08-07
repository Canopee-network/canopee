# Tutorial: sharing one identity across devices (laptop, phone)

This is a hands-on, step-by-step guide to a specific goal: the same
`IdentityId` — same signing key, same object ownership, same contacts'
trust — usable from more than one device, so a user isn't a different
person on their laptop than on their phone.

No code is given for Step 2 onward — each step describes what to build,
which existing functions to hook into, and how to verify it worked before
moving to the next step. Read
[`canopee-identity/README.md`](../crates/canopee-identity/README.md) and
[`end-to-end-encryption.md`](end-to-end-encryption.md) first — this
tutorial reuses both the plain identity-file model and the X25519
key-agreement primitive.

## The problem, concretely

Canopee has **no account layer above the private key**. `IdentityId`
(`canopee://identity/<peer-id>`) is derived directly from an Ed25519
public key — there's no server-side account it points at, no recovery
phrase, no "sign in with" flow. The identity *is* the key file
(`Identity::create`/`load` in
[`crates/canopee-identity/src/identity.rs`](../crates/canopee-identity/src/identity.rs)),
sitting at a fixed path under a per-device `Config` root (`~/.canopee/`
on a plain node, or an app-specific data directory for an embedded Tauri
app per
[`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md#step-1-make-config-support-a-custom-root)).

That means, as things stand:

- **Two fresh installs are two different people.** Install the same app
  on a laptop and a phone with no extra step, and `Identity::create` runs
  twice, producing two different keypairs and two different `IdentityId`s.
  Contacts who trust one don't automatically trust the other.
- **The only way to be "the same identity" on both is for both devices to
  hold the literal same private key file.** There is no other mechanism.
  Not "linked devices sharing a root of trust" (that's a harder design,
  covered in the stretch step below) — literally the same secret bytes,
  copied.

This tutorial covers the honest, buildable version of that: how to copy
the key safely, what you give up by doing it, and — as a stretch —
what a real multi-device design would need instead.

## Step 0: orient yourself in the existing code

Read these before changing anything:

- `Identity::create`/`Identity::load` in
  [`crates/canopee-identity/src/identity.rs`](../crates/canopee-identity/src/identity.rs)
  — notice `load` does no validation beyond "is this a well-formed
  protobuf-encoded keypair." It has no idea whether the bytes it's loading
  came from this device's own prior `create` call or were copied in from
  somewhere else. This is exactly the property that makes device-sharing
  possible with zero new code in `canopee-identity` itself — and exactly
  why nothing there will stop you from doing it unsafely.
- `Config::identity_path()` in
  [`crates/canopee-config/src/lib.rs`](../crates/canopee-config/src/lib.rs)
  — the fixed location `create`/`load` read and write. Two devices running
  independent `Config` roots (the normal case) each have their own file at
  this path; sharing an identity means making both paths hold identical
  bytes, not making them point at the same physical file (they're on
  different machines).
- [`canopee-identity/README.md`](../crates/canopee-identity/README.md#key-agreement-x25519) —
  the X25519 `dh_public_key()`/`agree()` primitive you'll use in Step 2 to
  build an encrypted transfer channel, instead of relying on whatever
  ad-hoc channel Step 1 uses being trustworthy by default.

**Checkpoint:** explain, in one sentence, why copying the *file* is
sufficient — i.e. why `canopee-network`'s `PeerId` and `canopee-storage`'s
object-ownership checks can't tell the difference between "the original
device" and "a second device holding a copy of the same key."

## Step 1: the honest v1 — manual export/import over a channel you already trust

**Goal:** get the exact bytes of `~/.canopee/identity/identity.key` (or
your Tauri app's equivalent, per its custom `Config` root) from device A
onto device B, using a transfer channel you already trust for other
sensitive data (a cable, AirDrop/Nearby Share on the same local network, a
password manager's secure notes) — explicitly **not** email, chat, or
general-purpose cloud sync, none of which you'd use for an SSH private key
either, and this is the same class of secret.

**What to build:** nothing new in the identity crate — this step is
process, not code, though a real app should still add a UI affordance for
it rather than telling users to find a file on disk themselves:
- An "export identity" action that reads the raw key file bytes (e.g. a
  Tauri command wrapping `tokio::fs::read(config.identity_path())`) and
  hands them to whatever OS-level sharing mechanism you're using (a save
  dialog, a share sheet).
- An "import identity" action on the second device that takes those same
  bytes and writes them to *that device's* `identity_path()` — but only if
  no identity already exists there. Overwriting an existing identity
  silently is a real footgun: a device that already has its own identity
  and then gets a different one copied over loses access to anything it
  had signed as the old one. Require an explicit confirmation, and
  consider backing up the old file rather than deleting it outright.

**Design question:** should storage (`~/.canopee/storage`, the objects
the device has locally) come along too, or only the identity? Recommend
**identity only** — each device keeps its own `Storage` directory and
independently fetches/caches whatever objects it needs from the network
(exactly how [`p2p-app-caching-tutorial.md`](p2p-app-caching-tutorial.md)
already expects any node to behave). Trying to also sync the storage
directory turns this into a much bigger, unrelated problem (two devices
concurrently writing object files with no locking), for no real benefit —
canopee already assumes objects are fetchable from the network, not that
a device is the only copy of anything it owns.

**How to verify:** export from device A, import on device B (two real
devices, or two separate `Config` roots on one machine if you don't have
a second device handy — same pattern as
[`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md#step-1-make-config-support-a-custom-root)'s
two-`Runtime`-in-one-process test). Confirm both sides report the *same*
`identity.id()` — same `canopee://identity/<peer-id>` string — and that an
object signed on device A (`identity.sign(...)`) verifies successfully
using device B's loaded copy of the same key
(`identity.verify(...)`). That's the core claim: both devices are
cryptographically indistinguishable as the same identity.

**What this does not give you**, and shouldn't be presented as solving:

- **No revocation.** If either device's copy is later compromised or
  lost, there's no protocol-level way to invalidate it — only the same
  social, out-of-band mitigation already described in
  [`security-considerations.md`](security-considerations.md#key-management):
  tell your contacts to stop trusting that `IdentityId`. Losing a phone
  with this key on it is exactly as bad as losing a laptop with it — worse,
  in the sense that phones are lost/stolen more often and this doc doesn't
  change that risk at all.
- **No device-scoped trust.** Both devices are fully, equally privileged —
  sign, publish, dial, everything `NodeCommand` exposes (see
  [`security-considerations.md`](security-considerations.md#local-trust-boundary-the-nodes-socket)).
  There's no "this device can read but not sign" tier.
- **Simultaneous use is untested territory, not validated-safe.** Since
  the same keypair also derives the libp2p `PeerId`
  ([`canopee-identity/README.md`](../crates/canopee-identity/README.md#why-one-keypair-for-both-roles)),
  running both devices on the network at the same time means two swarms
  presenting the same peer identity from two network locations
  simultaneously. Nothing in [`canopee-network`](../crates/canopee-network/README.md)
  was designed or tested with this in mind — it may work, degrade oddly
  under Kademlia routing, or behave unpredictably; treat it as unverified
  rather than assuming it's fine.

## Step 2: stop trusting the transfer channel — encrypt the export

**Goal:** the same result as Step 1, without requiring an already-trusted
physical channel (AirDrop, a cable) — useful when the two devices aren't
in the same room, or you don't want to rely on the user picking a safe
transfer method correctly every time.

**Where:** the X25519 primitive from
[`canopee-identity`](../crates/canopee-identity/README.md#key-agreement-x25519) —
but notice the problem this step actually has to solve first: `agree()`
needs *both sides already having an identity* to derive a shared secret,
and the whole point here is that device B doesn't have one yet. You can't
use the long-term identity keys for this — you need a **fresh, ephemeral**
DH keypair generated just for the pairing exchange itself.

**What to build**, a minimal pairing flow:
1. Device B (no identity yet) generates a throwaway X25519 keypair — not
   via `Identity`, just `x25519_dalek::StaticSecret::random()` directly —
   and displays its public key as a QR code (short-lived, single-use;
   discard it after import completes or after a timeout).
2. Device A scans the QR code, generates its own throwaway X25519
   keypair, and runs Diffie-Hellman against B's ephemeral public key to
   get a one-time shared secret — the same primitive
   `Identity::agree()` wraps, just applied to ephemeral keys instead of
   long-term ones (a KDF step first, same as
   [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md#6b-derive-a-symmetric-key-dont-use-agrees-output-directly)'s
   Step 6b — never use raw DH output as a cipher key).
3. Device A AEAD-encrypts the identity key file bytes under that one-time
   key and transmits the ciphertext back to device B — over the same QR
   channel if it fits (identity keys are small, ~a few dozen bytes plus
   protobuf overhead, likely QR-encodable directly), or over a short-lived
   local network connection the two devices negotiate however you like
   (this part isn't prescribed by anything in canopee — it's your app's
   pairing UX, not a network-layer concern).
4. Device B decrypts, writes the result to its own `identity_path()`, and
   discards the ephemeral keypair.

This is the same shape as
[`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md#step-6-end-to-end-message-encryption)'s
Step 6 (ephemeral/derived key agreement → KDF → AEAD encrypt), applied to
a one-time key transfer instead of an ongoing conversation — no new
cryptographic idea, just a different thing being protected.

**Design question:** does this actually need to be more secure than Step
1's "AirDrop it" in practice? For most users, a channel they already trust
for other sensitive transfers is simpler and just as safe as building a
whole pairing flow. Build Step 2 if your app's threat model specifically
includes "the two devices are never physically near each other and the
user has no other trusted channel" — otherwise Step 1 alone may be the
right stopping point, and it's fine to say so rather than building
machinery nobody needs.

**How to verify:** run the pairing flow between two devices with no other
channel available (e.g. disable AirDrop, don't allow a cable), confirm
device B ends up with a working, verified copy of the identity (same test
as Step 1's verification), and confirm — by inspecting whatever transport
carries the ciphertext — that the identity key bytes never crossed the
wire in plaintext at any point.

## What this tutorial does not solve: real multi-device identity

Everything above shares **one secret across devices** — it does not give
you *multi-device identity* in the sense Signal or Matrix mean it (each
device has its own key, independently revocable, all trusted as
"belonging to this person" without any of them holding a shared secret).
That's a substantially different and harder design:

- Each device would generate its **own** keypair on first run (the normal,
  unmodified `Identity::create` path — no copying).
- Some mechanism would let devices **cross-sign each other** — e.g. device
  A, once paired with device B (using something like Step 2's ephemeral
  DH exchange to establish a trusted channel), signs a small
  `DeviceLink { device_a_id, device_b_id }` record with its own key,
  and B does the same for A. Contacts who resolve either `IdentityId`
  would need a way to discover the *set* of device identities linked to a
  person, not just one.
- **Losing a device becomes revocable independently** — sign a
  `DeviceUnlink` record with any of your remaining devices' keys, and
  contacts who see it stop trusting the lost device's `IdentityId`,
  without needing to abandon the whole identity the way a single-key
  compromise forces today (per
  [`security-considerations.md`](security-considerations.md#key-management)'s
  "No revocation" note — this is the first real answer to that gap, scoped
  to the multi-device case specifically).

None of `DeviceLink`, `DeviceUnlink`, or a "which devices belong to this
person" resolution mechanism exist anywhere in canopee today — no
`ObjectType`, no DHT record shape, nothing. This is real, substantial
follow-up design work, closer in scope to `AppPointerRecord`'s original
design than to anything you can wire up in an afternoon. Worth treating as
its own future doc once (or if) shared-secret copying (Steps 1–2 above)
turns out not to be good enough for a real product.

## Summary checklist

- [ ] Step 1 — manual export/import of the raw identity key file over an
      already-trusted channel, verified by both devices reporting the same
      `IdentityId` and cross-verifying a signature
- [ ] Step 2 (optional) — an encrypted pairing flow using ephemeral X25519
      keys, for when no already-trusted channel exists between the two
      devices
- [ ] A documented decision on whether simultaneous multi-device use is
      supported, tested, or explicitly discouraged — don't leave this
      silently unverified
- [ ] Stretch — cross-signed per-device keys with independent revocation,
      if shared-secret copying turns out not to be good enough

By the end of Step 1 alone, a user can move their identity from a laptop
to a phone and be recognized as the same person on both — at the cost of
zero device-level revocation or trust separation, which is an honest
limitation to state, not a bug to silently ship around.
