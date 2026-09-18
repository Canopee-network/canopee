# Identity

Canopee has a clear separation between **who you are** (your identity) and
**which machine you happen to be on** (your device). This split is what makes
one identity many devices — and many apps — without collisions.

## The person: Identity

- One **Ed25519** identity keypair per user.
- Stored at `~/.canopee/identity/identity.key` (shared across *all* your
  devices and apps on this machine).
- The public key is the root of everything you sign: objects, pointers,
  profiles, capability grants.
- Its `IdentityId` is `canopee://identity/<peer-id-from-identity-key>`.
- Optional at-rest encryption: if `CANOPEE_IDENTITY_PASS` is set, the key is
  stored password-encrypted and unlocked on node start.

Every policy statement in Canopee is signed by your identity key:
- every `Object` you publish,
- every record pointer `(owner, name) → object`,
- every capability grant,
- your username claim and device registration.

This means anything carrying your signature is **verifiable by anyone** who
knows your `IdentityId` — no trusted server required. `Object::verify()` and
`PointerRecord::verify()` in `canopee-storage` implement this.

## The machine: DeviceKey

- One **Ed25519** keypair per device, stored at
  `~/.canopee/identity/device.key`, **never** shared or synced.
- The device key's public key is the machine's network `PeerId`.

Why a separate device key? Because the network peer id is a *transport
identity* — it can change, be rotated, or be ephemeral. Your person identity
must be stable, but your machine identity must be freely replaceable without
invalidating your objects, records, or signatures. If a device is lost or
compromised you rotate only the *device* key (remove the device from
`(owner, "devices")`), and your identity — and every object signed by it —
remains untouched.

## Mapping between the two

Two records keep the two worlds in sync (see [Objects & pointers](objects.md)):

| Record | Key | Value | Purpose |
|---|---|---|---|
| Device list | `(owner, "devices")` | signed `DeviceList` | the authoritative "which machines am I on" list; every device re-registers itself on startup |
| Device registry | `device:<network-peer-id>` (DHT) | canonical owner (`canopee://identity/<id>`) | reverse lookup: a stranger sees your device on the network → resolves to your identity |

Both are signed and verified, so a device cannot impersonate an owner it
doesn't hold keys for.

## Usernames

A username is a human-friendly handle:
- claimed per identity via `canopee identity claim <name>` (the CLI runs
  `claim_username`); stored in `(owner, "username")` plus a global
  `username:<lowercased-name>` Kademlia registry entry that maps name →
  verified identity.
- Usernames are unique network-wide and case-insensitive (normalized to
  lowercase).
- Anyone can `canopee resolve <username>` (or `resolve_username`) to find the
  owning identity without knowing its peer id up front.

See [Usernames guide](../guides/usernames.md) for the walk-through.

## Pairing

Pairing lets a new device become a recognized member of an existing identity
_owner-controlled_ and _verified_:

- `canopee pair` on the device you trust starts a short-lived session; it
  proves ownership by having **both** the account key and a session code.
- The pairing payload is encrypted with a session key derived (ECDH/X25519)
  between the two parties, so the secret never transits in plaintext.
- `canopee pair <session-code>` on the new device completes it, and afterwards
  the new device holds a copy of the identity key and joins the device list.

The pairing wire protocol lives in `canopee-network` (`PairingRequest` /
`PairingResponse` in `behaviour.rs`); the runtime side is in
`crates/canopee-runtime/src/pairing.rs` ([initiate_pairing],
[complete_pairing], [accept_pairing], [send_pairing_request]).

See [Multi-device identity guide](../guides/multi-device-identity.md).

## Profiles and contacts

Two person-level records round out the identity model:

- `(owner, "profile")` → signed `Profile` with a display name, an X25519 DH
  public key (so apps can derive E2E conversation keys without extra
  lookups), an optional avatar object id, and a version counter.
- `(owner, "contacts")` → signed `ContactList`; a shared, replaceable list of
  people you know (their `IdentityId`s), usable by any app.

Both are re-shared whenever the node starts (see
[Sharing](sharing.md)).

## Device sync

Because each device carries its own key, data written on one device needs to
reach the others. `canopee sync` (periodically, and on demand via
`sync_with_peer` / `sync_with_all_devices`):

1. resolves the owner's device list,
2. fetches the owner's public records (profile, devices, home) from each
   device,
3. fills missing object copies locally.

This is how an object put on your phone can be read on your laptop — both are
members of the same identity, so both can serve the owner's public objects.