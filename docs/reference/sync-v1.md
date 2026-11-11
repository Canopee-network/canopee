# Canopee Sync v1

Status: **M1 + M2 core implemented** (2026-10-27), including content-defined
chunking for large files. The remaining M2 hardening, scale tests, and the
production key model are still pending. This document is both the product
contract and the current implementation status; where they differ, the
[implementation status](#implementation-status) and
[known deviations](#known-deviations-from-the-v1-contract) win.

This is the product contract for Canopee's first production product: private
folder sync across a person's own devices.

## Product definition

Canopee Sync keeps a folder identical across the devices of one Canopee
identity. A user creates a sync root on one device, joins it on another, and
edits files normally. Canopee transports encrypted changes; it never puts
plaintext file contents or path names on the network.

v1 is deliberately narrow:

- One identity per person, multiple trusted devices.
- Full-folder sync. No selective sync, sharing, collaboration, or public links.
- An always-on **primary role** is required for v1 availability. “Primary” is
  a role, not a device class: a desktop, NAS, or mini-PC is recommended, but a
  phone may serve as primary where the OS permits reliable background work.
  iPhones generally cannot, so the phone should be an authorized peer or
  optional backup rather than the only primary.
- An optional **backup** device stores ciphertext and the latest indexes.
  It is not a relay for live edits and never receives plaintext.
- Headless daemon and CLI ship first; the desktop app follows the proven engine.

The local folder is plaintext because it is the user's real working copy.
Everything Canopee stores or sends is encrypted client-side.

## Non-goals for v1

- Folder sharing with other people or other identities.
- Mobile clients, web clients, or a hosted SaaS control plane.
- Real-time collaborative editing or per-file permissions.
- Deduplication across identities.
- Being a backup product for a device that never syncs while online.
- Preserving open file handles, ACLs, extended attributes, or symlinks.

## Security invariants

These are release blockers, not best-effort goals. The current implementation
does not satisfy all of them; see the status notes after each item.

1. Relay, bootstrap, backup, and storage peers cannot read file contents,
   file names, directory names, or sync-root names. **Partially met:** folder
   contents, paths, and root names are inside encrypted objects; object sizes
   and the fixed `(owner, "sync")` pointer remain visible.
2. Only devices authorized in the current device set can decrypt new content
   or produce accepted manifests. **Not met:** pairing copies the account
   identity key to every device.
3. A revoked device cannot decrypt content encrypted after revocation.
   Content it already downloaded cannot be taken back; the UI and docs must
   state this. **Not met:** independent device revocation is impossible today.
4. Every manifest is signed by an authorized device and bound to the account,
   root, epoch, and revision. The DHT pointer is a hint, never trust.
   **Partially met:** objects are identity-signed, but there is no device
   certificate, epoch, or `previous` field yet.
5. Object IDs for encrypted chunks are keyed, so an attacker cannot confirm
   guessed plaintext by hashing it. **Partially met:** content-defined chunking
   is implemented, but chunk ids are ordinary encrypted-object ids derived
   from random per-object envelopes rather than keyed plaintext ids.
6. No plaintext file data or path names appear in `~/.canopee`, backups,
   logs, metrics, or error messages. **Partially verified:** the e2e test
   checks object stores, not logs/metrics.
7. Revocation and key rotation do not re-encrypt history in place; they
   protect future content. History is protected by the fact that old keys are
   removed from the device set, not by retroactive deletion. **Not applicable
   until revocation exists.**

## Identity, device, and key model

### Current interim model

The current model copies the account identity key to every paired device.
This works for development and lets every device decrypt owner-encrypted
objects, but it makes independent revocation impossible. It is not acceptable
for production file sync.

### Required production model

Canopee Sync v1 requires a delegation migration:

- **Account root key** (`R`): long-lived Ed25519 signing key. Its public key is
  the existing Canopee identity. It lives on the primary and in the user's
  recovery export, not on every device.
- **Device keys**: each device has an Ed25519 signing key and an X25519 key
  agreement key. A signed device certificate binds both public keys to the
  account, device id, capabilities, and key epoch.
- **Device set**: a root-signed, monotonically versioned list of device
  certificates. Non-primary devices may sign manifests only if their
  certificate is present in the current set.
- **Root content key** (`K_root`): random 256-bit key per sync root, used to
  derive file/chunk keys and the keyed chunk identifier. It is wrapped to each
  authorized device's X25519 key.
- **Root index key** (`K_index`): random key used to encrypt the account's
  `name -> root_id` index. It is wrapped alongside `K_root`.

Signing keys are not reused for Diffie-Hellman. X25519 device keys are
separate from Ed25519 identity/device signing keys.

### Pairing and joining

Pairing authorizes a device by adding its certificate to the device set. The
primary (or a recovery flow) signs the updated set, increments the epoch,
rotates `K_root`/`K_index`, and wraps the new keys to every remaining device.
Joining a root only creates a local path mapping; it does not create a new
remote root.

### Revocation

Revocation must be possible from the primary or recovery export:

1. Remove the device certificate and increment the device-set epoch.
2. Generate new `K_root` and `K_index` values for each root.
3. Wrap the new keys to all remaining devices.
4. Publish new encrypted indexes/manifests under the new epoch.

A revoked device keeps any plaintext already on its disk and any ciphertext it
already downloaded. It must not be able to fetch or decrypt anything encrypted
after the epoch change. This limitation is explicit in the UI.

### Recovery

The account root key must be exportable as a recovery secret. Losing every
authorized device without a recovery export means losing the account. v1 must
document and test recovery before GA.

## Data model

### Implemented today

```text
account sync index — encrypted object behind (owner, "sync")
  version: u64
  roots: [ { name, root_id, manifest: ObjectId, revision, updated_at } ]

sync manifest — encrypted object
  root_id: String
  revision: u64
  entries: [ { path, object: ObjectId, size, content_hash, modified_at } ]
  tombstones: [ { path, deleted_at, content_hash: Option<[u8; 32]> } ]

sync file index — encrypted SyncFile object, referenced by a manifest entry
  format: u32
  size: u64
  content_hash: [u8; 32]
  chunks: [ { object: ObjectId, size: u64, content_hash: [u8; 32] } ]

device-local mapping — ~/.canopee/sync/roots.bin
  roots: [ { name, root_id, path, base_manifest, base_revision, last_sync_at } ]
```

- The index, manifests, file indexes, and chunks are encrypted to the identity.
  The DHT exposes only the fixed `(owner, "sync")` pointer and opaque object
  ids/sizes.
- Files are split with content-defined chunking (target 1 MiB, minimum 256 KiB,
  maximum 4 MiB) and each chunk is an encrypted `Blob`. A manifest entry points
  at a `SyncFile` index; `content_hash` is SHA-256 over the whole plaintext and
  lives inside both the encrypted manifest and the encrypted index. Manifest
  entries written before chunking still point directly at a whole-file `Blob`
  and remain readable.
- A tombstone records the content hash the deleting device believed it was
  deleting. If a stale delete races a newer local edit, the hashes differ and
  the edit survives instead of being removed.
- Directories are implicit from file paths. There are no directory entries,
  modes, ACLs, or extended attributes.
- Paths are relative, slash-separated strings. Validation rejects absolute
  paths, backslashes, NUL/control characters, and empty, `.` or `..`
  components. Symlinks and non-regular files are skipped.
- `modified_at` is Unix seconds; `deleted_at` and conflict timestamps are Unix
  milliseconds.
- The local mapping stores the last applied manifest and revision so a pass can
  distinguish local changes, remote changes, and conflicts.

### Target v1 model

```text
account sync index (encrypted, signed)
  roots: [ { name, root_id, created_at } ]

sync root
  root_id: random 32-byte identifier (public only as a pseudonym)
  name: user-visible, encrypted in the index
  current_manifest: object id + revision + epoch

manifest (encrypted, signed by an authorized device)
  root_id, epoch, revision, previous, device_id, created_at
  entries: path -> file | dir | tombstone
  file: size, mtime, mode, chunk ids, content hash
  tombstone: deleted_at, previous revision

chunk
  keyed id, ciphertext, nonce, plaintext length
```

The target adds content-defined chunking (target 1 MiB, min 256 KiB, max
4 MiB), keyed chunk ids, per-root keys, directory entries, a `previous`
revision link, and device/epoch binding. Compression, if enabled, happens
before encryption. No cross-root dedup.

## Sync behavior

### Create and join

`sync create <path> --name <name>` creates a root, writes the initial
manifest, publishes the encrypted index, and records the local mapping.
`--name` defaults to the directory name.

`sync join <name> <path>` resolves the existing root from the encrypted index,
then records a device-local path mapping. The destination may be missing,
empty, or already contain files: existing local files are preserved and join
the root like any other local additions. It fails if the root is unknown,
already joined on this device, or the path is already mapped to another root.
A path that exists remotely and locally with different content is handled as a
conflict, not silently overwritten.

### Change propagation

Implemented:

1. A recursive filesystem watcher triggers a pass about 500 ms after local
   changes, while the node is started and has at least one connected peer. An
   explicit `sync now` runs a pass immediately. A 30-second periodic pass is the
   fallback when watcher events are missed and also requires a connected peer.
2. Each pass rescans the whole local folder with a streaming SHA-256, uploads
   changed files as content-defined encrypted chunks plus a `SyncFile` index,
   fetches missing remote chunks/indexes, and publishes an updated encrypted
   manifest if anything changed. Chunk reuse is keyed by chunk hash and size
   from the previous local manifest entry, so an edit only uploads changed
   chunks. Peak file-buffer memory is bounded by the 4 MiB maximum chunk size.
3. Remote files are applied through a temporary file in the destination
   directory, `fsync`, atomic rename, then a best-effort parent-directory
   `fsync` on Unix.
4. Before applying and before publishing, the device re-resolves the sync
   record. If another device published a newer manifest, the pass retries (up
   to five attempts) instead of overwriting it.
5. A restart re-marks locally owned sync objects as servable, so a device can
   still serve its manifests/files after the in-memory shared set is rebuilt.

Not implemented yet:

- Crash/power-loss injection coverage and multi-GiB scale tests.
- A fully serialized multi-writer manifest chain; optimistic re-resolve/retry
  narrows but does not eliminate concurrent-publisher races. The tombstone
  content hash prevents a stale delete from removing a newer local edit.

### Conflicts and deletes

Implemented behavior:

- If both sides changed the same path, the remote manifest wins at the
  original path and the local version is preserved alongside it as
  `name.ext.conflict-<UTC-millis>`. The conflict copy is uploaded and published
  in the same sync pass.
- If one side deletes and the other modifies, the modified file survives: a
  local modification wins over a remote delete, and a remote modification wins
  over a local delete.
- Deletes are tombstones carrying the content hash they believed they were
  deleting. They are retained for 30 days (wall clock) so a stale device cannot
  resurrect a deleted file, then pruned on the next manifest publish. This is
  metadata GC only; old chunk objects are not collected yet.
- Automatic merges of file contents are out of scope for v1.

### Availability

v1 requires at least one always-on device with the **primary role** for
devices that are not online at the same time. The role can run on any suitable
device; it is not tied to a device class. If every device is offline, changes
queue locally; there is no hosted relay in v1. The backup device can serve
ciphertext to authorized devices but is not part of the live-edit availability
guarantee. This must be stated plainly in the product UI.

## CLI surface

The record refresh remains under `sync records`.

```text
canopee sync create <path> [--name <name>]
canopee sync join <name> <path>
canopee sync list
canopee sync status [name]
canopee sync now [name]
canopee sync remove <name>
canopee sync records [peer-id]
```

- `create` creates a remote root and local mapping.
- `join` maps an existing remote root to a local path.
- `remove` removes the local mapping only; it does not delete remote data or
  the folder.
- `list` / `status` currently report name, local path, revision, and last-sync
  timestamp. Pending-change/conflict/key-epoch reporting is still future work.
- `now` returns per-root uploaded/downloaded/removed/conflict counters.
- All commands work against the daemon over the existing node socket, so the
  desktop app can use the same operations.

## Implementation status

### M1

M1 delivered the smallest correct engine behind this contract:

- Whole-file encrypted `Blob` objects.
- Encrypted root index at `(owner, "sync")`; manifests and files encrypted to
  the identity's devices.
- Create/join, initial upload/download, nested files, and bidirectional edits.
- Explicit `sync now` plus a 30-second periodic pass when connected.
- Identity key copied by pairing as an interim model.

### M2 core

M2 added correctness under normal offline/restart use:

- Tombstones in addition to file entries; deletes propagate in both directions.
- Tombstone content hashes so a stale delete cannot remove a newer local edit.
- Tombstones are pruned 30 days after `deleted_at` on the next manifest publish.
- Local-modification-vs-remote-delete and remote-modification-vs-local-delete
  rules.
- Conflict copies, uploaded and published in the same pass, without duplicates
  on later passes.
- Content-defined chunking (1 MiB target, 256 KiB minimum, 4 MiB maximum),
  streaming hashes/scanning, chunk reuse, and per-chunk integrity checks.
- Legacy whole-file `Blob` manifest entries remain readable while new uploads
  use `SyncFile` indexes.
- Atomic remote apply with temp file, `fsync`, rename, and best-effort parent
  directory `fsync`.
- Optimistic index re-resolve and retry before applying/publishing.
- Restart re-marking of locally owned sync objects and chunks as servable.
- Recursive watcher with ~500 ms debounce; periodic/explicit passes remain.
- Acceptance coverage for restart, offline edits, deletes, conflicts,
  delete-vs-edit, no resurrection after restart, and a multi-chunk file.

### Remaining M2 hardening

- Scale tests, including a multi-GiB file without unbounded memory.
- Crash/power-loss injection during upload/download.
- Object/chunk garbage collection for data no longer referenced by any manifest.
- Stronger multi-writer serialization than optimistic retry.

## Known deviations from the v1 contract

| Contract | Today |
|---|---|
| Content-defined 1/256K/4M chunks | Implemented; legacy whole-file blobs still read |
| Keyed chunk ids and `K_root`/`K_index` | One identity envelope per chunk/object; ids not plaintext-keyed |
| Device certificates, epochs, revocation | Account key copied to every device |
| Manifest `previous`, `epoch`, `device_id`, directories/modes | Only `root_id`, `revision`, files, tombstones |
| Serializable multi-writer chain | Optimistic retry; no CAS/lease |
| Backup/restore product path | Not implemented |
| Desktop UI | Not implemented |

## Milestones

- **M0 — specification and tests.** This document, security invariants, and
  the first two-device acceptance test.
- **M1 — headless single-root sync.** Done: create/join, initial upload,
  initial download, nested files, bidirectional edits, no plaintext in stores.
- **M2 — correctness under real use.** Core done: offline edits,
  restart/resume, deletes, conflict copies, atomic apply, no resurrection,
  content-defined chunking, bounded per-chunk memory, 30-day tombstone pruning,
  and a ~500 ms filesystem watcher. Hardening pending: crash recovery and scale
  tests.
- **M3 — production security and backup.** Device delegation, revocation,
  key rotation, recovery export, ciphertext backup, restore.
- **M4 — desktop app.** UI over the daemon: root setup, status, conflicts,
  pause/resume, open folder.
- **M5 — hardening and beta.** Cross-platform packaging, scale, observability,
  upgrade/migration, incident playbooks.

## Acceptance tests

The current acceptance test (`two_nodes_sync_folder` in
`crates/canopee-e2e/tests/e2e.rs`) proves:

1. Two paired devices converge on a created root with nested files,
   including a multi-megabyte file split into several chunks.
2. Both directions propagate creates and edits.
3. Joining preserves pre-existing local files.
4. Local and remote deletes propagate.
5. An offline edit against a remote edit preserves both versions as exactly
   one original plus one conflict copy, published in the same sync pass; later
   passes do not duplicate it.
6. Delete-vs-edit keeps the edit in both directions.
7. A tombstone survives a restart and does not resurrect a deleted file.
8. Neither device's object store contains plaintext file content or a
   plaintext chunk probe.

Later acceptance tests must cover:

- Crash/power-loss injection during upload/download; no partial destination
  files and the previous manifest remains good.
- A revoked device cannot decrypt new manifests or chunks.
- A backup node cannot decrypt file contents, names, or the root index.
- A relay sees only opaque objects.
- 10,000 files and a 10 GiB file complete without unbounded memory use.
- Power-loss simulation preserves the previous good manifest.

## Production gate

No public production release until all of these are true:

- Device delegation/revocation and recovery are implemented and tested.
- No plaintext path names or contents in stores, backups, logs, or metrics.
- Crash consistency is tested, not assumed.
- macOS, Linux, and Windows have passed the same acceptance suite.
- The always-on-primary requirement is visible in onboarding and docs.
- Upgrade and identity-migration paths from current paired identities exist.
