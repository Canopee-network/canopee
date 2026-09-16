# Tutorial: sharing one identity across devices (laptop, phone)

This is a hands-on, step-by-step guide to a specific goal: the same
`IdentityId` — same signing key, same object ownership, same contacts'
trust — usable from more than one device, so a user isn't a different
person on their laptop than on their phone.

Multi-device identity is now **built into canopee**: every device gets its
own per-device key, and a pairing flow transfers the shared account key to
a new machine over the LAN. This tutorial walks through the built-in flow,
verifies what it actually gives you, and states honestly what it still
doesn't solve. A manual encrypted export/import fallback (for when the two
devices are on different networks) and the SDK/embedded-app flow are
covered as steps too.

Read [`canopee-identity/README.md`](../crates/canopee-identity/README.md)
first — it explains the account-key vs. device-key split this whole doc
builds on.

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

Since the device-key work, two fresh installs are still two different
*people* (each mints its own account key on first run) — **but** a device
is no longer fused to the identity the way it used to be:

- Each machine mints its own long-lived **device key** (`DeviceKey`, a
  libp2p keypair in
  [`crates/canopee-identity/src/device.rs`](../crates/canopee-identity/src/device.rs)).
  Its public key is the device's network `PeerId` — distinct from the
  account key, so several devices of one identity can be online at once as
  separate peers.
- The account key is shared onto a new machine **once, deliberately**, via
  LAN pairing. After that the two machines are "the same person" but
  clearly *different devices*: same `IdentityId`, different `PeerId`s.

This tutorial covers the flow that makes that happen, how to verify it
worked, and — as honest warnings — what still isn't solved.

## Step 0: orient yourself in the existing code

Read these before changing anything:

- `Identity::create`/`Identity::load` in
  [`crates/canopee-identity/src/identity.rs`](../crates/canopee-identity/src/identity.rs)
  — the *account* key. Notice `load` does no validation beyond "is this a
  well-formed protobuf-encoded keypair." That's exactly why the identity
  can be transferred between devices at all — and why the pairing flow,
  not the key parser, is what gates trust.
- `DeviceKey` in
  [`crates/canopee-identity/src/device.rs`](../crates/canopee-identity/src/device.rs)
  — the *device* key. One per machine, never transferred; its public key
  is the libp2p `PeerId`.
- `Config::identity_path()` and `Config::device_key_path()` in
  [`crates/canopee-config/src/lib.rs`](../crates/canopee-config/src/lib.rs)
  — the two fixed files every node reads and writes. The identity file is
  the one pairing moves; the device key file is the one that stays.
- The pairing protocol types in
  [`crates/canopee-protocol/src/lib.rs`](../crates/canopee-protocol/src/lib.rs)
  (`PairingQrData`, `PairingData`, `PairingRecord`, …).

**Checkpoint:** explain, in one sentence, why two devices running the same
`IdentityId` but different device `PeerId`s are *not* the "two swarms,
same identity, same network address" collision the old single-key model had.

## Step 1: pair two devices on the same LAN (built into the CLI)

**Goal:** get the identity from the device that already carries it onto a
fresh device that doesn't, using the built-in pairing flow. Both must be
reachable on the same LAN; the source device's node must be running.

On the **new device** (no identity yet):

```bash
canopee pair
```

This prints a **12-character pairing code** and a base64 **QR payload**.
The code is what the *person* will type on the other device to approve —
it never travels over the wire. The QR payload encodes the new device's
`PeerId`, name, and its advertised LAN dial address.

On the **source device** (the machine already carrying the identity):

```bash
canopee pair <qr-payload> --code <the-12-char-code>
```

Typing the code is the explicit-approve step that makes pairing safe
against a rogue device reaching the wire. The source then:

1. derives a session key from the code + one-time session id + both device
   ids (`derive_session_key`, Argon2id-style KDF over the code — see
   `canopee_identity::pairing`);
2. encrypts its **account key plus its signed user records** (device list,
   profile, contacts) under that key (`encrypt_pairing_payload`);
3. dials the new device's advertised LAN address and delivers the payload
   over `/canopee/pairing/1.0.0` (`send_pairing_request`).

On the receiving side (`accept_pairing` in
[`crates/canopee-runtime/src/lib.rs`](../crates/canopee-runtime/src/lib.rs)),
every check is strict before anything is persisted:

- the payload must target this device's pending session (single-use — the
  session is consumed whether or not the request is valid);
- the `from` peer must match the actual sender;
- the AEAD must decrypt under the code-derived key (a wrong or absent code
  fails here);
- every transferred record must verify against the transferred identity.

Then the identity is written to this device's key file — honoring the same
at-rest policy as normal startup (`CANOPEE_IDENTITY_PASS` → encrypted),
with the previous key (if any) backed up as
`identity.key.bak-<timestamp>` rather than deleted. Records are imported
and cached. The source device then reports the transfer was accepted.

**Important:** the current runtime keeps running on its temporary
identity. The transferred one **takes effect on the next restart** — the
CLI reminds you of this.

**How to verify:** after restarting, on both devices run:

```bash
canopee identity     # same canopee://identity/<id> on both
canopee device       # different peer id + device name on each
canopee devices      # both machines listed on the source's identity
```

The core claim — both devices are cryptographically the *same identity*
but clearly *different devices* — is proven by those three outputs
matching/mismatching exactly the way the last two lines said.

## Step 2: pairing from an embedded app or the SDK

If your app embeds the runtime in-process (the Tauri pattern) or talks to
a node over the SDK, the same flow is two method calls:

- **New device** — `Runtime::initiate_pairing()` (or
  `CanopeeClient::pair_initiate()` via the `InitiatePairing` command):
  mints a fresh code + session id and returns the `PairingQrData` to
  display. The node keeps the code in memory only.
- **Source device** — `Runtime::complete_pairing(qr, code)` (or
  `CanopeeClient::pair_complete(...)` via the `CompletePairing` command):
  verifies the typed code against the QR, encrypts the payload, dials,
  delivers.

The pieces the old "build your own pairing flow" write-ups assumed you'd
hand-roll are all here now — with the same design the tutorials
recommended:

- an **ephemeral one-time session** per pairing (fresh code + session id),
  discarded on accept/restart;
- the code is **never on the wire** — it only derives the session key, so
  a passive eavesdropper on the LAN sees ciphertext it can't decrypt;
- the session key also binds **both device ids** in its salt, so a payload
  stolen for one pairing can't be replayed at another.

There is no QR *scanner* built in — the CLI prints the payload as base64
and you can read it, scan it with your own decoder, or wire the
`PairingQrData` into your app's own QR UI. The transport that carries the
QR payload between machines is deliberately yours to choose (that's a UX
decision, not a network-layer one).

## Step 3: after pairing — device list and sync

Devices register themselves in the identity's public `(owner, "devices")`
record the moment a runtime starts (`register_device`), and each device
publishes a `device:<peer-id> → canopee://identity/<id>` registry record
so other peers can reverse-resolve "whose device is this."

Since all your devices share the same DHT keys, they publish into the same
records, so the meaningful sync problem is: **which copy of profile /
contacts / devices is newest?** The answer is last-writer-wins over the
DHT:

- `canopee sync` — refresh this node's user records from every registered
  device;
- `canopee sync <peer-id>` — refresh from one specific peer.

A background task also refreshes automatically every 30s while the node is
online with at least one connected peer (`spawn_periodic_sync`). Use
`canopee sync` to trigger it manually and to see whether any record
actually changed (`profile/contacts/devices updated` vs. `Already up to
date`).

## Fallback: manual export/import (no LAN, or a custom transfer channel)

Pairing needs both devices on the same network. When they aren't — or if
you're building your own transfer UX on top of an already-trusted channel
(cable, AirDrop, a secure note) — canopee exposes the raw move:

```bash
# On the source device:
canopee export-identity --passphrase 'my-passphrase' --output identity-export.bin

# Move identity-export.bin to the target device however you like, then:
canopee import-identity identity-export.bin --passphrase 'my-passphrase' [--overwrite]
```

Notes, exactly as with pairing:

- the passphrase is a **transfer** passphrase, not a recovery secret — it
  protects the exported envelope in transit and at rest on the destination
  (it uses the same `export_encrypted` envelope machinery as encrypted
  identity-at-rest);
- the imported key **takes effect after a node restart**, and `--overwrite`
  backs up the existing key rather than deleting it;
- unlike pairing, this does **not** copy your profile/contacts/device
  records or register the new device — run `canopee sync <source-peer-id>`
  (or `canopee sync`) after to pull those over the network.

## What this still does not solve

- **No independent per-device revocation of the account key.** A device
  can be dropped from the `(owner, "devices")` list (via the
  `RemoveDevice` command / SDK), and after that peers stop dialing it — but
  a stolen *device* copy of the shared account key still holds perfectly
  valid signing keys. There is no "revoke just this machine" mechanism,
  and there is no way to revoke the account key at all without telling
  your contacts to stop trusting the whole `IdentityId` (the same social
  mitigation as in [`security-considerations.md`](security-considerations.md#key-management)).
- **No delegation and no tiers.** All devices are fully, equally
  privileged — sign, publish, dial, everything `NodeCommand` exposes (see
  [`security-considerations.md`](security-considerations.md#local-trust-boundary-the-nodes-socket)).
- **Cross-device sync is last-writer-wins.** Concurrent edits from two
  devices can be lost; there's no conflict resolution, only "newest signed
  pointer wins."
- **No account recovery.** If every copy of the account key is lost, the
  identity is gone. Pairing and export/import are key *transfer*, not
  key *backup*.

Everything above shares **one account secret across devices** — it is not
"each device has its own key, independently revocable, all trusted as
belonging to this person" in the Signal/Matrix sense. That stronger design
(per-device keys **cross-signed into the account**, independent device
revocation) remains future work: today each device does have its own
signing key, but trust in that key comes from membership in the owner's
`(owner, "devices")` list, not from a per-device verifiable certificate.
Until that exists, "one secret, many machines, remove by listing" is the
honest model — a real improvement over copying the key file with no
device tracking at all, and still a documented limitation, not a solved
one.

## Summary checklist

- [ ] Pair two devices on the same LAN with `canopee pair` (code on the
      new device, QR + typed code on the source), restart the new device
- [ ] Verify `canopee identity` matches across devices, `canopee device`
      differs, and `canopee devices` lists both machines
- [ ] Decide how pairing is triggered in your app (CLI vs. `initiate_pairing`
      / `complete_pairing` via the SDK) and what carries the QR payload
      between machines
- [ ] If devices may be on different networks, choose the manual
      `export-identity` / `import-identity` fallback + post-import `sync`
- [ ] Decide what removing a device means in your app and document it
      (list removal ≠ revocation of a stolen account-key copy)

By the end of Step 1 alone, a user can move their identity from a laptop to
a phone — same `IdentityId`, per-device identity intact, device list
tracking both — and the honest limitations above are still limitations to
state, not bugs to silently ship around.