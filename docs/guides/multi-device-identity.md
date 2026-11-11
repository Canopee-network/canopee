# Multi-device identity

Bring one identity to a second device (laptop → phone, desktop → laptop) and
keep both in sync. This guide walks through the built-in flow end to end.

Read [Identity concept](../concepts/identity.md) first — the
account-key-vs-device-key split this whole guide builds on.

## Why transfer is even possible

Canopee has **no account layer above the private key**. `IdentityId`
(`canopee://identity/<peer-id>`) is derived directly from an Ed25519 public
key — no server, no recovery phrase. The identity *is* the key file at
`~/.canopee/identity/identity.key`; the device additionally mints its own
per-device key at `identity/device.key` whose public key is the network
`PeerId`.

So two fresh installs are two different *people* — until you deliberately
share the account key onto the second machine once, via pairing. After that:
**same `IdentityId`, different `PeerId`s**, and both can be online at once.

## Step 1: pair two devices on the same LAN

Both devices must be on the same LAN and the source device's node must be
running.

**On the new device** (no identity yet):

```bash
canopee pair
```

This prints a **12-character pairing code** and a base64 **QR payload**. The
code is what the person types on the other device to approve — it never
travels over the wire. The QR payload encodes the new device's `PeerId`,
name, and its advertised LAN address.

**On the source device** (already carrying the identity):

```bash
canopee pair <qr-payload> --code <the-12-char-code>
```

Typing the code is the explicit approval. The source then:

1. derives a session key from the code + one-time session id + both device
   ids (Argon2id-style KDF, see `canopee_identity::pairing`),
2. encrypts its **account key plus its signed user records** (device list,
   profile, contacts) under that key,
3. dials the new device's advertised LAN address and delivers the payload.

On the receiving side every check is strict before anything is persisted:
the payload must target this device's pending single-use session, the sender
must match, the AEAD must decrypt under the code-derived key (a wrong code
fails here), and every transferred record must verify against the
transferred identity.

The identity is written to the device key file (honoring
`CANOPEE_IDENTITY_PASS` at-rest policy, previous key backed up as
`identity.key.bak-<timestamp>`). Records are imported and cached.

> **The transferred identity takes effect on the next restart** — the
> runtime you paired from is still running on its temporary identity.

**Verify:**

```bash
canopee identity    # same canopee://identity/<id> on BOTH devices
canopee device      # different peer id + device name on EACH
canopee devices     # both machines listed on the source's identity
```

## Step 2: after pairing — device list and record sync

Devices register themselves in the `(owner, "devices")` record the moment a
runtime starts (`register_device`), and each publishes a
`device:<peer-id> → identity` registry record so peers can reverse-resolve
"whose device is this."

User records (profile, contacts, devices, home index) are refreshed from the
DHT and are **last-writer-wins**:

```bash
canopee sync records              # refresh records from every registered device
canopee sync records <peer-id>    # refresh from one specific peer
```

A background task also syncs records periodically while the node is online
(`spawn_periodic_sync` in `crates/canopee-runtime/src/sync.rs`). This does not
make arbitrary stored objects shared; normal serving rules still apply.

## Step 3: folder sync (optional)

To keep an actual directory in step, use Canopee Sync. On the first device:

```bash
canopee sync create ~/Documents --name docs
```

On another paired device:

```bash
canopee sync join docs ~/Documents
canopee sync now docs
canopee sync status docs
```

`join` preserves files already in the destination and uploads them as local
additions. The encrypted index/manifests live behind `(owner, "sync")`; local
folder mappings live in `~/.canopee/sync/roots.bin`.

Folder sync uses a recursive watcher with ~500 ms debounce while the node is
started and connected; a 30-second periodic pass is the fallback, and
`canopee sync now` forces one.
If both devices edit the same path, one version wins at the original path and
the other is kept as `<name>.conflict-<UTC-millis>`. See
[Canopee Sync v1](../reference/sync-v1.md) for the remaining gaps.

## Fallback: manual export/import (no LAN)

When the two devices aren't on the same network — or you prefer your own
transfer channel (cable, AirDrop, a secure note) — use the raw move:

```bash
# source device:
canopee export-identity --passphrase 'my-passphrase' --output identity-export.bin

# transfer identity-export.bin however you like, then on the target:
canopee import-identity identity-export.bin --passphrase 'my-passphrase' [--overwrite]
```

Notes, as with pairing: the passphrase is a **transfer** passphrase
(protecting the exported envelope), the imported key takes effect after a
restart, and `--overwrite` backs up the old key rather than deleting it.
Unlike pairing, export/import does **not** copy your profile/contacts or
register the new device — run `canopee sync records` afterwards to pull those
over the network.

## Embedded apps / SDK

The same flow is two method calls:

- **New device** — `Runtime::initiate_pairing()` /
  `CanopeeClient::pair_initiate()`: mints a code + session id, returns
  `PairingQrData` to display.
- **Source device** — `Runtime::complete_pairing(qr, code)` /
  `CanopeeClient::pair_complete(...)`: verifies the code, encrypts the
  payload, dials, delivers.

There's no built-in QR *scanner* — the CLI prints base64; wiring
`PairingQrData` into your own QR UI is up to you (a UX choice, not a
network one).

## What this still does not solve

- **No independent per-device revocation of the account key.** Removing a
  device from `(owner, "devices")` stops peers dialing it, but a stolen
  *device copy* of the shared account key still has valid signing keys.
- **No delegation / tiers.** All devices are equally privileged.
- **User-record sync is last-writer-wins.** Concurrent record edits can lose.
  Folder sync instead keeps both same-path versions as an original plus a
  conflict copy, but it does not merge file contents, and concurrent manifest
  publishers are protected only by optimistic re-resolve and retry.
- **No account recovery.** Lose every copy of the key, lose the identity.
  Pairing and export/import are key *transfer*, not key *backup*.

The honest model today: **one secret, many machines, remove-by-listing** —
a real improvement over a copied key file with no device tracking, and still
a documented limitation, not a solved one.

## Summary

1. `canopee pair` on the new device → code + QR.
2. `canopee pair <qr> --code <code>` on the source → approve + transfer.
3. Restart the new device; verify `identity` matches, `device` differs,
   `devices` lists both.
4. Keep user records fresh with `canopee sync records`.
5. Optionally create/join a folder root with `canopee sync create` /
   `canopee sync join`.
6. Different networks → `export-identity` / `import-identity` + `sync records`.