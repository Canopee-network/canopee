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
must be stable, but your machine identity should be replaceable without
invalidating your objects, records, or signatures.

Important limitation: pairing still copies the **account identity key** to
each device. Removing a device from `(owner, "devices")` stops peers dialing it
but does not revoke its copy of the account key. Independent device
revocation requires the delegated-key migration in
[Canopee Sync v1](../reference/sync-v1.md#identity-device-and-key-model).

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

- `canopee pair` on the **new** device starts a single-use session and prints a
  12-character code plus a QR payload.
- On the existing device, `canopee pair <qr> --code <code>` is the explicit
  approval. The code never travels over the wire.
- The payload is encrypted with a key derived from the code, the one-time
  session id, and both device ids (Argon2id → XChaCha20-Poly1305), so a wrong
  code fails authentication.
- After approval the new device holds a copy of the account identity key and
  joins the device list.

The pairing wire protocol lives in `canopee-network` (`PairingRequest` /
`PairingResponse` in `behaviour.rs`); the runtime side is in
`crates/canopee-runtime/src/pairing.rs` (`initiate_pairing`,
`complete_pairing`, `accept_pairing`, `send_pairing_request`).

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

## Record sync

Because each device also has its own device key, user records written on one
device need to reach the others. `canopee sync records` (periodically, and on
demand via `sync_with_peer` / `sync_with_all_devices`):

1. resolves the owner's device list,
2. fetches the owner's user records (profile, contacts, devices, home) from
   each device,
3. fills missing object copies locally.

Record refresh is last-writer-wins. It does not turn arbitrary stored objects
into shared ones; normal sharing/serving rules still apply.

## Folder sync

Folder sync is a separate path for keeping a **local directory** in step
across your devices:

```bash
canopee sync create ~/Documents --name docs   # one device
canopee sync join docs ~/Documents            # another device
canopee sync now docs
```

The account's `name -> root` index is encrypted at `(owner, "sync")`; manifests
and file blobs are encrypted objects. A recursive watcher triggers a pass about
500 ms after local changes while the node is started and connected, a 30-second
periodic pass is the fallback, and `sync now` forces a pass.

Record sync is last-writer-wins; folder sync preserves both sides of a
same-path edit as an original plus a `.conflict-<UTC-millis>` copy. Deletes use
tombstones. See [Canopee Sync v1](../reference/sync-v1.md) for the exact
semantics, known gaps, and production requirements.