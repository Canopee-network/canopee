# Plan: Multi-Device Identity Sync

**Milestone**: Multiple devices, one identity
**Date**: 2026-09-14

---

## Design Decisions

| Decision | Choice |
|---|---|
| Multi-device scope | Shared secret now, structured for per-device keys later |
| Data sync scope | Identity + contacts + profile only; objects fetched on-demand from P2P |
| Sync transport | P2P direct via libp2p |
| Mobile target | iOS + Android via Tauri 2 |
| Pairing method | QR code scan **OR typed 12-char code** (both LAN-only) |
| Pairing network | LAN only — mDNS + probe for discovery; no DHT, no relay, no internet |
| PeerId collision | Per-device Ed25519 keypairs (Device IDs) |
| Conflict resolution | Last-writer-wins (version field) |
| Device key storage | Separate file `~/.canopee/identity/device.key` (i.e. `identity_path().join("device.key")`) |
| QR code content | `{device_id, device_name, lan_addr, code}` — no ephemeral X25519 pubkey needed |
| Pairing trust | 12-char base32 code (60 bits), hash-keyed, never transmitted in full, mixed into KDF; source must give an explicit approve step before sending the identity copy |
| Pairing protocol | One unified libp2p request-response protocol `/canopee/pairing/1.0.0` |
| Sync direction | Existing device (source) sends encrypted identity → new device (target) |

---

## Architecture Overview

### Identity Model

```
~/.canopee/identity/
├── identity.key      # Shared signing key (Ed25519) - SAME on all devices
└── device.key        # Device-specific key (Ed25519) - UNIQUE per device
```

- **Identity key**: Signs objects, records, profiles. Shared across all devices. The `IdentityId` (`canopee://identity/<peer-id>`) is derived from this key.
- **Device key**: Derives the libp2p `PeerId` for network addressing. Unique per device. Never leaves the device.
- **Device ID**: `DeviceId = PeerId(device_key)`. Published in a `DeviceList` record signed by the identity key.

### Sync Records

New signed records in `canopee-storage/src/user.rs`:

| Record | Purpose |
|---|---|
| `RECORD_DEVICES = "devices"` | List of `(device_id, device_name, added_at)` entries, signed by identity key |

### Pairing Flow (QR or typed code, LAN-only)

Both devices must be on the same LAN. The new device is found via mDNS (or a
typed probe), then pairing runs over one unified request-response protocol
(`/canopee/pairing/1.0.0`). The 12-char base32 code (60 bits) is hash-keyed
(never sent in full) and mixed into the KDF; the ephemeral-session key is
discarded after pairing.

```
Device B (new)                         Device A (existing, LAN)
─────────────                          ─────────────────────────
1. Generate device keypair
2. Show QR or code: {
     device_id: PeerId(device_key),
     device_name: "iPhone",
     lan_addr: addr,
     code: "…12 chars…"
   }
                                       4. Scan QR or type code
                                       5. Approve the pairing
                                         (what's shared: identity,
                                          profile, contacts)
                                       6. Confirm code ↔ start session
                                       7. KDF(shared_secret) → aes_key
                                       8. AEAD encrypt(identity_key_bytes +
                                         device_list + profile + contacts,
                                         aes_key)
                                       9. Send ciphertext to Device B
10. Decrypt with same KDF
11. Write identity.key, device.key
12. Store profile, contacts locally
13. Sign + publish own DeviceList entry
14. Discard ephemeral session key
                                       15. Receive confirmation
                                       16. Discard ephemeral session key
```

Typed-code pairing is usable from both the CLI and the Tauri app; the QR path
is the same flow with the QR carrying `{device_id, device_name, lan_addr, code}`.

### Ongoing Sync Protocol

After pairing, devices sync via libp2p:

1. Device B resolves `(identity, "devices")` record to find Device A's PeerId
2. Device B dials Device A via the existing object exchange protocol
3. Device B fetches: profile, contacts, home index (via `resolve_pointer` + `fetch_object`)
4. Last-writer-wins: compare `version` fields, keep the higher one

No new libp2p protocol needed for ongoing sync — compose existing `resolve_pointer` + `fetch_object` primitives. (The **pairing** handshake is the one new protocol: `/canopee/pairing/1.0.0`.)

---

## Implementation Phases

### Phase 1: Identity Export/Import (Foundation) — ✅ DONE

**Goal**: Move an identity key between devices as encrypted bytes.

**Status**: fully implemented across all layers + tested (identity unit tests,
runtime behavior, and a real two-node e2e test in `canopee-e2e`). Design notes
(frozen during implementation):

- Import is *never* a silent overwrite: it requires `overwrite: bool`; the
  previous key file is backed up to `identity.key.bak-<ts>`.
- The node's at-rest policy is preserved: if `CANOPEE_IDENTITY_PASS` is set,
  the imported key is re-encrypted under that passphrase before writing.
- Importing a key identical to the current one is a no-op (no restart).
- Import writes to disk and requires a **node restart** to adopt the key:
  the live runtime is already bound to its current key (ownership of storage,
  the running swarm) and cannot hot-swap.

#### 1.1 Identity crate changes

**File**: `crates/canopee-identity/src/identity.rs`

- Add `Identity::export_bytes(&self) -> Result<Vec<u8>>` — raw key bytes
- Add `Identity::export_encrypted(&self, passphrase: &str) -> Result<Vec<u8>>` — encrypted envelope
- Add `Identity::import_from_encrypted(bytes: &[u8], passphrase: &str) -> Result<Self>` — decrypt + load
- Tests: round-trip, wrong-passphrase rejection, garbage rejection (13 tests total)
- `DeviceKey` is Phase 2

#### 1.2 Config changes

**File**: `crates/canopee-config/src/lib.rs`

- Add `Config::device_key_path() -> PathBuf` — returns `identity_path().join("device.key")` (+ test)

#### 1.3 Protocol commands

**File**: `crates/canopee-protocol/src/lib.rs`

Add to `NodeCommand`:
```rust
ExportIdentity { passphrase: String },
ImportIdentity { bytes: Vec<u8>, passphrase: String, overwrite: bool },
```

Add to `NodeResponse`:
```rust
IdentityExported { bytes: Vec<u8> },
IdentityImported { identity_id: IdentityId },
```

#### 1.4 Runtime methods

**File**: `crates/canopee-runtime/src/lib.rs`

- Add `Runtime::export_identity(&self, passphrase: &str) -> Result<Vec<u8>>`
- Add `Runtime::import_identity(&self, bytes, transfer_passphrase, overwrite) -> Result<IdentityId>`

#### 1.5 Node handler

**File**: `crates/canopee-node/src/lib.rs`

Handle new `NodeCommand` variants in the request handler.

#### 1.6 SDK client

**File**: `crates/canopee-sdk/src/client.rs`

- Add `CanopeeClient::export_identity(passphrase) -> Result<Vec<u8>>`
- Add `CanopeeClient::import_identity(bytes, passphrase, overwrite) -> Result<IdentityId>`

#### 1.7 CLI commands

**File**: `crates/canopee-cli/src/main.rs`

- `canopee export-identity --passphrase <p> --output <file>`
- `canopee import-identity <file> --passphrase <p> [--overwrite]` — prints the
  imported `IdentityId` and "restart the node to adopt it".

#### 1.8 Gateway commands

**File**: `crates/canopee-gateway/src/protocol.rs` (+ `lib.rs` dispatch)

- `GatewayCommand::ExportIdentity { passphrase }`
- `GatewayCommand::ImportIdentity { data_b64, passphrase, overwrite }`

**Verification**: ✅ e2e test `two_nodes_export_and_import_identity` — export
from root A, import into fresh root B; B refuses without `--overwrite` and with
the wrong passphrase; after a node restart both report the same `IdentityId`.
(Signing an object on A and verifying on B is covered once Phase 2 gives each
device its own PeerId.)

---

### Phase 2: Device Keys — ✅ DONE

**Goal**: Each device has its own PeerId derived from a device-specific keypair, so multiple devices of one identity can be online at once without colliding.

#### 2.1 DeviceKey module

**File**: `crates/canopee-identity/src/device.rs` (new, exported from `lib.rs` as `DeviceKey`)

- `DeviceKey::generate(device_name)` — in-memory fresh keypair.
- `DeviceKey::create(path, name)` — writes to disk (protobuf, like Identity).
- `DeviceKey::load(path, name)` — reads from disk.
- `DeviceKey::load_or_create(path, name)` — race-safe first-provisioning (`create_new` write with retry-on-conflict, mirroring `Identity::create_if_absent`). Two apps provisioning the same user root converge on one key.
- `keypair()` / `peer_id()` / `device_name()` accessors.
- The device name defaults to the machine hostname; override via the `CANOPEE_DEVICE_NAME` environment variable (wired in `Runtime::default_device_name`).

#### 2.2 NetworkManager change

**File**: `crates/canopee-network/src/manager.rs`

`NetworkManager::new()` now accepts a `Keypair` (the device keypair) instead of `Arc<Identity>`. The swarm's peer id is the device's peer id. The legacy `peer.identity = Some(canopee://identity/<peer-id>)` inference on `ConnectionEstablished` / `IdentifyReceived` has been **removed** — after device keys a peer id is a device id, not an identity, so identity is resolved through the registry (see 2.5).

#### 2.3 Storage additions

**File**: `crates/canopee-storage/src/user.rs`

| Constant / Type | Purpose |
|---|---|
| `RECORD_DEVICES = "devices"` | `(owner, "devices")` pointer name — the authoritative "which machines carry my identity" register. |
| `DEVICE_REGISTRY_PREFIX = "device:"` | DHT mutable record key prefix: `device:<peer-id>` → canonical identity string. Reverse-maps a device peer id back to its owner; used by `enrich_peers` to show identity metadata in `canopee peers`. |
| `DeviceEntry { device_id, device_name, added_at: Option<OffsetDateTime> }` | One device currently carrying an identity. |
| `DeviceList { devices, version }` | The signed object published under `(owner, "devices")`. |

#### 2.4 Runtime integration

**File**: `crates/canopee-runtime/src/lib.rs`

- `Runtime` gains a `pub device_key: Arc<DeviceKey>` field.
- `Runtime::open_with_config()` creates or loads the device key before building `NetworkManager` and passes `device_key.keypair()` to it.
- `Runtime::default_device_name()` — hostname fallback, overridable via `CANOPEE_DEVICE_NAME`.
- On startup (`register_device`): upserts this device into the local `DeviceList`, persists + announces the list object, and publishes both the `(owner, "devices")` pointer and the `device:<id>` → identity registry record to the DHT. A background task (`schedule_device_publish`) re-publishes those DHT records after the first peer connection is established, so peers can resolve the device mapping even if the swarm wasn't ready at open time.

#### 2.5 Device resolution

- `resolve_device_identity(device_id) -> Option<IdentityId>` — resolves a device's peer id back to its owner via the `device:<id>` registry (local cache → DHT, bounded by `RESOLVE_DHT_TIMEOUT`).
- `resolve_device_peer_id(owner) -> Option<PeerId>` — resolves an owner's identity string to the peer id of its first registered device. For the local node this is always this machine's own device id. For remote nodes it reads `(owner, "devices")` via `resolve_owner_object` (pointer + network fetch if not cached).
- `enrich_peers()` rewritten: resolves each connected peer's identity through the device registry (with a legacy fallback for pre-device-key peers), then resolves username + profile as before. Peers whose identity is resolvable through the registry show in `canopee peers`; peers whose identity is derived via the legacy assumption keep showing only the raw short peer id (no `Identity:` line).

#### 2.6 Protocol, Node, SDK, CLI additions

| Layer | What was added |
|---|---|
| `canopee-protocol` | `NodeCommand::{Device, DeviceList, ResolveOwnerDevice, AddDevice, RemoveDevice}`, `NodeResponse::{Device, DeviceList, OwnerDevice, DeviceAdded, DeviceRemoved}`, `DeviceInfo` struct. |
| `canopee-node` | Dispatch arms for all five new commands. |
| `canopee-sdk` | `CanopeeClient::{device, device_list, resolve_owner_device}` methods. |
| `canopee-cli` | `canopee device` (prints this device's peer id + name) and `canopee devices` (lists the account's registered devices). `resolve_peer_arg` updated: identity/username arguments now resolve to a **device** peer id (via `resolve_owner_device`); falls back to the legacy identity-bound peer id for pre-device-key peers. |

#### 2.7 Tests

- `canopee-identity` DeviceKey tests: reload round-trip, concurrent race safety, distinct paths mint distinct keys.
- `canopee-storage` DeviceList round-trip test (serialize → verify → decode).
- `canopee-network` handshake tests updated to device keypairs (two_nodes_dial_and_discover, node_fetches, pubsub).
- `canopee-runtime` isolated-roots test updated to tolerate the auto-registered device record.
- `canopee-sdk` client integration test updated: `list()` now includes the device record object.
- `canopee-e2e` both tests rewritten for device keys:
  - `two_nodes_share_and_unshare_end_to_end` — discovery via device peer id, fetch-by-username flowing through `resolve_owner_device` (identity → device → dial), find-providers asserted against device peer id.
  - `two_nodes_export_and_import_identity` — after restart, asserts distinct device peer ids, same identity, each node listing its own device; identity-bound peer id != device peer id.

#### 2.8 Known limitations / next steps

- **Device list merge semantics are not coordinated.** Each device publishes its own `(owner, "devices")` list at startup; last-writer-wins on the DHT mutable record. A multi-device user may see only one device's entry until they both connect and re-publish. Phase 3 (pairing) will re-register both devices explicitly, which naturally fixes this.
- **`resolve_peer_arg` legacy fallback**: when the owner's device list isn't resolvable, the CLI falls back to the identity-bound peer id (which is NOT dialable for Phase-2 peers), resulting in a clear dial error. This is acceptable for now; a retry-across-devices strategy is a future improvement.
- **`enrich_peers` per-peer timeout**: `RESOLVE_DHT_TIMEOUT` is 10s but the outer `ENRICH_TIMEOUT` of 5s bounds each peer, so slow DHT lookups are truncated. The device registry record is fast on the same LAN but can take up to 10s over a relay connection.

---

### Phase 3: LAN Pairing (QR + typed code)

**Goal**: Pair a new device to an existing identity over the LAN, via QR scan
or a typed 12-char code.

> **Status: implemented end-to-end (Sep 2026).** Flow shipped: CLI `canopee pair`
> with no args prints `Pairing code: <12 chars>` + a bincode/`base64` QR payload;
> `canopee pair <payload> --code <code>` on the source device verifies the typed
> code, dials the new device over the LAN, and delivers the encrypted identity +
> signed records over `/canopee/pairing/1.0.0`. Verified by the e2e test
> `two_nodes_pair_over_lan` (identity adopts exactly on restart, device key/PeerId
> preserved, device lists merged). The wrong-code rejection happens at the source
> before anything leaves the machine. Deviations from the spec below:
> - The **code itself is never transmitted**: only `(session_id, device_id)`
>   go on the wire; the source compares the typed code locally (constant-time)
>   and mixes it into the KDF (`salt = b"canopee/pairing/v1" || session_id ||
>   source_device || new_device`). An attacker replaying the wire message cannot
>   decrypt without knowing the code.
> - Payload sections were implemented as a typed `PairingData { version,
>   identity_key: Vec<u8>, records: Vec<PairingRecord> }` (bincode), where each
>   record names a transferred app record (devices/profile/contacts) with its
>   signed `Object` + `AppPointerRecord`; the receiving side re-verifies every
>   signature and owner before importing.
> - Profile+contacts transfer uses the on-disk pointer files + local record
>   cache rather than the DHT.
> - QR = `PairingQrData` bincode→base64 (this is what `pair` prints); the typed
>   12-char code IS one of its fields, so the payload carries the code — see
>   `pair_complete` which re-checks it on the source. The LAN addr field
>   (`lan_addr`) drives the source's dial.
> - Remaining niceties (not required for the verified flow): QR rendered as an
>   actual image, Tauri UI for pairing, auto-redial backoff (a dial race was seen
>   once; a rerun passed).

#### 3.1 Pairing protocol types

**File**: `crates/canopee-protocol/src/lib.rs`

```rust
pub struct PairingQrData {
    pub version: u8,
    pub device_id: String,        // PeerId of new device
    pub device_name: String,
    pub lan_addr: String,         // how the source device can reach it
    pub code: String,             // 12-char base32 pairing code (hash-committed)
}

pub struct PairingPayload {
    pub encrypted: Vec<u8>,  // AEAD-encrypted, length-prefixed sections
}
```

#### 3.2 Pairing logic in runtime

**File**: `crates/canopee-runtime/src/lib.rs`

- `Runtime::initiate_pairing(&self) -> Result<PairingQrData>` — new device:
  generates device keypair, returns QR/code data (keeps secret material in memory)
- `Runtime::complete_pairing(&self, qr: PairingQrData) -> Result<PairingPayload>` —
  existing device: verifies the code, encrypts identity + profile + contacts +
  device list, returns payload
- `Runtime::accept_pairing(&self, payload: PairingPayload) -> Result<()>` —
  new device: verifies code, decrypts, imports identity, stores records

#### 3.3 Crypto flow

- 12-char base32 code (60 bits); a **hash of the code** is exchanged (the code
  itself is never transmitted in full) and mixed into the KDF
- KDF → 32-byte session key
- AEAD: XChaCha20-Poly1305 (same as identity encryption)
- Payload sections (length-prefixed): `identity_key_bytes || device_list_bytes || profile_bytes || contact_list_bytes`

#### 3.4 Transport

- Discovery by mDNS on the LAN; probe/connect via `/canopee/pairing/1.0.0`
- The source (existing device) shows an explicit **approve** step before the
  identity copy is sent
- Typed-code pairing exposed through CLI and Tauri; the QR path uses the same
  flow with `PairingQrData` bincode→base64 encoded into the QR (~100 bytes)

**Verification**: Device A shows QR/code, Device B scans/types + approves.
Device B ends up with the same identity (IdentityId), profile, contacts.
Device B's PeerId differs from Device A's. Session secrets discarded after
pairing.

---

### Phase 4: Sync Protocol

**Goal**: Keep identity, profile, and contacts synchronized across paired devices.

> **Status: implemented (Sep 2026).** `canopee sync` refreshes the three user
> records (profile, contacts, devices) from the DHT, last-writer-wins by the
> signed pointer's `published_at`. The local record cache is bypassed so a
> fresh DHT fetch always runs; a newer pointer triggers an object fetch +
> cache update. Verified by the e2e test `two_nodes_sync_profile` (pair B onto
> A, restart B, A sets a profile, B syncs and sees it). Deviations from the
> spec below:
> - The explicit "dial each device" step is skipped: the DHT handles
>   discovery, and records are per-identity (not per-device), so a single
>   DHT fetch suffices. `sync_with_peer(peer_id)` accepts a peer hint for API
>   compatibility but the refresh is identical.
> - `SyncResult` lives in the protocol crate (the runtime uses it, the node
>   returns it over the socket).
> - Periodic background sync (§4.3) remains a follow-up; the CLI/SDK sync is
>   manual for now.

#### 4.1 Sync method in runtime

**File**: `crates/canopee-runtime/src/lib.rs`

```rust
pub struct SyncResult {
    pub profile_updated: bool,
    pub contacts_updated: bool,
    pub devices_updated: bool,
}

impl Runtime {
    pub async fn sync_with_peer(&self, peer_id: PeerId) -> Result<SyncResult> { ... }
    pub async fn sync_with_all_devices(&self) -> Result<SyncResult> { ... }
}
```

Flow:
1. Load own device list
2. For each device in the list (except self):
   a. Try to dial the device
   b. Resolve `(identity, "profile")`, `(identity, "contacts")`, `(identity, "devices")` — cross-check versions
   c. If remote version is newer → fetch and store locally (last-writer-wins)

#### 4.2 Protocol commands

**File**: `crates/canopee-protocol/src/lib.rs`

Add to `NodeCommand`:
```rust
SyncFromPeer { peer_id: String },
SyncDeviceList,
```

Add to `NodeResponse`:
```rust
SyncComplete { result: SyncResult },
```

#### 4.3 Periodic sync (follow-up)

Background task in `Runtime` using tokio interval. Only sync when online and connected.

> **Status: implemented (Sep 2026).** `spawn_periodic_sync()` runs every 30s,
> gated on `state.started` (online) and `network.peers() > 0` (connected).
> Verified by the e2e test `two_nodes_periodic_sync` (B never calls `canopee
> sync`; the background task fires and imports A's profile). One subtlety
> caught by the test: polling `canopee profile` populates the local cache via
> `resolve_pointer`, which makes the periodic sync see the cache as
> up-to-date — the test waits a fixed 45s instead of polling.

**Verification**: Two devices paired via QR. Edit profile on Device A. Device B syncs and sees updated profile. Edit contacts on Device B. Device A syncs and sees updated contacts.

---

### Phase 5: Tauri 2 Mobile Setup

**Goal**: Run canopee-home on iOS and Android with QR pairing and sync.

> **Status: 5.2 + 5.3 + 5.4 implemented (Sep 2026); 5.1 partially blocked.**
> - **5.3 Tauri commands** + **5.4 Pairing UI**: shipped in `apps/canopee-home`
>   (the renamed `canopee-chat-test`). Commands: `initiate_pairing`,
>   `complete_pairing`, `sync_now`, `get_identity`, `get_profile`,
>   `save_profile`. UI: identity/profile display, "Pair new device" (QR + code),
>   "Scan QR" (camera via `html5-qrcode` + paste-payload fallback), "Sync now".
>   Verified: app connects to a node via `CANOPEE_APP_ROOT` and queries identity.
> - **5.2 Camera scanning**: `html5-qrcode` QR scanner component with
>   paste-payload fallback (desktop). Camera path is mobile-only.
> - **5.1 Mobile config**: iOS project generated (`tauri ios init`), deployment
>   target 14.0. Rust compiles for `aarch64-apple-ios-sim`. **Blocked on the
>   iOS 26.5 simulator runtime download** (8.5 GB, in progress:
>   `xcodebuild -downloadPlatform iOS`). Android not set up (no Android SDK).
> - Deviations: `canopee-chat-test` → `apps/canopee-home`; `accept_pairing`
>   Tauri command dropped (automatic in the runtime); `canopee-sdk` dep switched
>   from remote git to local path; requires Node ≥ 20.12 for frontend build.
> - Remaining: on-device verification (iOS simulator once the runtime finishes
>   downloading, Android emulator once the SDK is installed).

#### 5.1 Tauri mobile configuration

**File**: `apps/canopee-home/src-tauri/tauri.conf.json`

Add mobile sections (android minSdk, iOS minimum deployment target), enable bundling for mobile targets.

#### 5.2 Camera/QR scanning

Frontend JS: `html5-qrcode` or `jsQR` library reading `navigator.mediaDevices.getUserMedia()`. QR generation: `qrcode` JS library rendering to canvas/SVG.

#### 5.3 Tauri commands

**File**: `apps/canopee-home/src-tauri/src/lib.rs`

Implemented commands (wrapping the existing SDK methods):
- `initiate_pairing` → `sdk.pair_initiate()` → returns `{ code, device_name, payload }` (payload = base64 bincode, for QR rendering)
- `complete_pairing(payload, code)` → decodes payload → `sdk.pair_complete(qr, code)` → returns status message
- `sync_now` → `sdk.sync_with_all_devices()` → returns `SyncResult`
- `get_identity` / `get_profile` / `save_profile` (for the profile UI)

#### 5.4 Pairing UI

**File**: `apps/canopee-home/src/App.tsx`

Screens: identity display, profile display/edit, "Pair new device" (shows QR + code), "Scan QR" (paste payload + type code), "Sync now" button. The QR is rendered with the `qrcode` npm package.

**Verification**: Build for iOS simulator and Android emulator. Desktop shows QR, mobile scans → mobile has identity + profile + contacts. Mobile's PeerId differs from desktop's. Edit profile on desktop → mobile syncs.

---

## File Change Summary

| Crate/File | Changes |
|---|---|
| `canopee-identity/src/device.rs` | `DeviceKey` struct + `generate`/`create`/`load`/`load_or_create` + 3 unit tests (new file) |
| `canopee-identity/src/lib.rs` | `pub use device::DeviceKey` (added export) |
| `canopee-config/src/lib.rs` | `device_key_path()` (Phase 1, unchanged) |
| `canopee-storage/src/user.rs` | `RECORD_DEVICES`, `DEVICE_REGISTRY_PREFIX`, `DeviceEntry`, `DeviceList` + round-trip test |
| `canopee-protocol/src/lib.rs` | `DeviceInfo`, `NodeCommand::{Device, DeviceList, ResolveOwnerDevice, AddDevice, RemoveDevice}`, matching `NodeResponse` variants |
| `canopee-runtime/src/lib.rs` | `device_key` field; `open_with_config` creates device key + registers device; `register_device`/`save_device_list`/`load_device_list`/`add_device`/`remove_device`/`resolve_device_peer_id`/`resolve_device_identity`/`publish_device_registry`/`publish_record`/`resolve_record`/`cache_record`/`schedule_device_publish`/`default_device_name`; `enrich_peers` rewritten for device→identity registry; `reshare_public_user_records` includes `RECORD_DEVICES` |
| `canopee-network/src/manager.rs` | `NetworkManager::new()` accepts `Keypair` (device keypair) instead of `Arc<Identity>`; `peer.identity` inference lines removed |
| `canopee-node/src/lib.rs` | Dispatch arms for all 5 new `NodeCommand` variants |
| `canopee-sdk/src/client.rs` | `device()`, `device_list()`, `resolve_owner_device()` |
| `canopee-cli/src/main.rs` | `Device`/`Devices` commands; `resolve_peer_arg` updated for identity→device resolution |

---

## Risks & Mitigations

| Risk | Mitigation |
|---|---|
| QR payload too large for camera scan | Identity payload is ~200 bytes base64; QR capacity is ~2-3 KB — fine |
| Simultaneous device edits cause data loss | Last-writer-wins with version field; documented limitation |
| Mobile background sync killed by OS | Sync-on-open is sufficient for v1; background sync is a follow-up |
| Device key file lost/corrupted | Remove device from device list via another device; re-pair |
| Identity key compromised on one device | Shared-secret model: all devices share the risk. Document that mitigation = re-key the whole identity |
| Bootstrapping: new device can't find existing device | Pairing is LAN-only: both devices must be online on the same LAN (mDNS/probe). After pairing, ongoing sync is over DHT/libp2p |
| Breaking change: PeerId derivation switches from identity key to device key | Existing single-device setups get a new PeerId. Need a migration path OR a flag: legacy mode uses identity-key PeerId when no device.key exists. Device list resolves the mixed case |

---

## Verification Checklist

- [x] Export identity from Device A, import on Device B → same `IdentityId` (e2e ✅)
- [x] Import refuses without `--overwrite` and with wrong passphrase (e2e ✅)
- [x] Import + restart → node adopts new identity; objects signed pre/post switch (e2e ✅)
- [x] Two devices with same identity, different PeerIds, both connect to network (e2e ✅)
- [x] Sign object on A, verify on B (e2e ✅)
- [ ] QR pairing: B scans A's QR → B has identity + profile + contacts
- [ ] Typed-code pairing via CLI and Tauri (LAN-only)
- [ ] Edit profile on A → sync → B sees updated profile
- [ ] Edit contacts on B → sync → A sees updated contacts
- [ ] Add third device via QR/code → device list updated on all three
- [ ] Remove device from list → lookup/connect to it stops
- [ ] Legacy single-device setup still works (no device.key → identity-key PeerId, or auto-migrate)
- [ ] Build for iOS simulator → runs
- [ ] Build for Android emulator → runs
- [ ] QR scan works on mobile camera
- [ ] E2E: desktop + mobile paired, sync profile, send message