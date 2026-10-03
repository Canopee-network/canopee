# HANDOFF / RESUME — Phase-1 "concat to any device" milestone

> Written 2026-09-24. A fresh session should read this FIRST, then re-verify
> anything it relies on before claiming a green. Every number below was real
> at time of writing; re-prove before trusting.

## Status: SHIPPED and GREEN, except one shelved item

| Layer | Verdict (real) |
|---|---|
| Storage gate | 22 passed (incl. tamper/mismatch/replay/atomicity) |
| Network gate | 15 passed (incl. Store/Replicate relay arm) |
| Runtime gate | 27 passed (incl. both concat e2e) |
| Serial e2e gate | **7 passed / 0 failed / 187.99s** — all 4 former DHT-pull flakes deterministic |
| Workspace release build | clean |
| Git | committed + pushed to `master` + relay redeployed on VPS |
| **Live relay** | `89.127.234.35:4001` **OPEN** (verified) |
| Release binaries | `target/release/{canopee-cli,canopee-node,canopee-edge}` present |

**The one unlanded item:** a TRUE cross-device store→retrieve→verify **through
the new VPS relay**. It was never honestly green because the previous session's
test inputs failed (used a Linux path on this macOS host), not the code. Do NOT
claim this arm until it produces real verdict output.

## Bare facts / contracts (read from source, don't guess again)

- Repo: `/Users/pierreportal/dev/CANOPEE/canopee` (case-sensitive; wrong-case fails). Remote: `git@github.com:Canopee-network/canopee.git`, branch `master`.
- Live relay: `/ip4/89.127.234.35/tcp/4001/p2p/12D3KooWEHG…` — get the full 52-char peer id from `crates/canopee-network/src/manager.rs` (`DEFAULT_BOOTSTRAP_ADDRS`, around line 185) instead of trusting this truncated copy.
- Env isolation (all real, from `crates/canopee-config/src/lib.rs` + `manager.rs`):
  - `CANOPEE_APP_ROOT` — per-app root (isolated homes). Default `~/.canopee`.
  - `CANOPEE_MDNS=0` — forces relay+DHT only (no LAN shortcut). This is the knob that makes a local run a HONEST cross-device stand-in.
  - `CANOPEE_BOOTSTRAP_ADDRS` / `CANOPEE_BOOTSTRAP_ADDRS_PREPEND` — override bootstrap addrs.
- CLI contracts (from `crates/canopee-cli/src/main.rs`):
  - `put <path>` (no `--name`; name derives from filename, `--ids` off by default)
  - `pair [QR_PAYLOAD] --code <12-char-code>` (no-arg `pair` prints QR + code)
  - `get <64-hex-id>` (also by name), `describe <id>`, `concat <ids…> --name <n>`, `share <name> [id]`, `fetch <peer> <id>`
- Node daemon: spawned by the CLI; a `pair`-ing new device adopts the owner identity over the relay circuit. `CANOPEE_MDNS=0` on BOTH sides = only shared path is the relay.

## Re-entry: the exact two-device acceptance to run (macOS host, real file)

```bash
cd /Users/pierreportal/dev/CANOPEE/canopee
BIN=target/release/canopee-cli; A=/tmp/canopee_devA; B=/tmp/canopee_devB
rm -rf $A $B; mkdir -p $A $B
# Device A — store a REAL darwin file, capture its Id
CANOPEE_APP_ROOT=$A CANOPEE_MDNS=0 $BIN put /etc/hosts
# Device A — start pairing (note: QR payload + 12-char code)
CANOPEE_APP_ROOT=$A $BIN pair
# Device B — same relay-only, pair onto A, then retrieve + verify A's object
CANOPEE_APP_ROOT=$B CANOPEE_MDNS=0 $BIN pair <QR_PAYLOAD> --code <CODE>
CANOPEE_APP_ROOT=$B CANOPEE_MDNS=0 $BIN get <THE_64_HEX_ID>
```

Report the ACTUAL verdict lines. Nothing is green until it is.

## Standing rules (do not violate)

- **Never commit / push / deploy without explicit user go.** User said only
  "shelve acceptance: give me fresh budget and re-run it." Shipping commands
  exist but require a fresh explicit OK.
- Do not paraphrase a green you didn't prove. Previous session's failures were
  its own test inputs, not the code — keep it that way.
- Rebuild/re-sign macOS binaries before deploy (`codesign -s - -f <bin>`; mac
  SIGKILLs unsigned edge builds with exit 137). Binaries historically
  `_binaries/…` at time of writing but the confirmed live path is
  `target/release/…`.

## What the milestone delivered (for the commit message / next session)

- Storage replay/verify engine hardening: tamper / key-mismatch / replay /
  atomic tmp+rename put — 22 tests.
- Network active Store-on-receive + replicate-to-connected-peers arm
  (`ObjectRequest::Store(ExportBundle)` / `Stored`), `replicate_object` /
  `ReplicateObjectToConnected`, `peers()` — 15 tests.
- Runtime `concat`: verifies each input, concatenates payloads into one
  verified Blob, stores + actively replicates to connected peers — 27 tests
  incl. both concat e2e, and pointer-record pull arm (connected-peers-first).
- CLI: `concat` subcommand; e2e gate 7/7 in 187.99s with all 4 former DHT-pull
  flakes deterministic.

## Next (only the one shelved item is pending)

1. Complete the true cross-device acceptance above. Then milestone Phase-1 is
   fully done and you may propose a Phase-2 (public-app/edge or next milestone
   from `PRODUCT_DEV_PLAN.md`) — but propose, don't auto-start.

---

# ADDENDUM — recipient-scoped E2E object encryption (2026-10-02)

Uncommitted work on top of the above. **Nothing here is committed or pushed.**
Re-verify before claiming green.

## What changed

Objects are now encrypted by default, end to end.

- `crates/canopee-identity/src/envelope.rs` (new). `CNP1` envelope: one random
  256-bit content key encrypts the payload with XChaCha20-Poly1305; that key is
  wrapped per recipient with ephemeral-static X25519, then HKDF-SHA256
  (`info = "canopee/object-wrap/v1" || epk || recipient_pub`, random 8-byte
  salt) and XChaCha20-Poly1305. Own DH key is always included.
- Why per-identity works for multi-device: pairing copies the *identity* key, so
  every device you own derives the same X25519 key. One key, both properties.
- `Runtime`: `seal()`, `open_object()`, `Runtime::is_encrypted()`,
  `put_object_for()` (explicit recipients), `put_object_public()`.
  `Runtime::get()` now returns plaintext; `fetch_object()` stays a raw
  storage primitive and returns ciphertext. `concat_objects` and app serving
  open objects first.
- Legacy plaintext objects still read: `is_envelope()` is a magic-prefix test,
  `decrypt_if_encrypted()` passes non-envelopes through. No migration step.
- `metadata.size` deliberately keeps the **plaintext** length (UI needs real
  sizes). That leaks file size — a known, documented tradeoff.

## Sharing model (user decision)

`share` publishes to the DHT with no named recipient, so an owner-only
encrypted object would be unreadable by everyone else. Chosen: **re-encrypt a
per-recipient copy on share.**

- `canopee share <name> [id] --to <contact>` (repeatable). Empty `--to` keeps the
  existing id; with recipients the published copy gets its **own id**, because
  objects are addressed by their ciphertext. Your local original stays private.
- `--to` resolves a contact **name or peer id** to their DH key from your
  contact list. New: `canopee contact-add <name> <peer-id> <dh-key-b64>` and
  `canopee dh-key` (your own key, base64).
- Protocol: `NodeCommand::ShareObject` gained `recipients: Vec<[u8; 32]>`;
  `NodeCommand::PutObjectPublic` is new; `NodeCommand::DhPublicKey` is new.

## Two bugs found and fixed while doing this

1. **`canopee profile <name>` published an all-zero DH key** when no profile
   existed. That makes an identity unshareable: peers wrap the content key for a
   key nobody holds, and decryption fails with no obvious cause. Now
   `Runtime::save_profile` always stamps the live `identity.dh_public_key()`, so
   CLI/gateway/SDK callers cannot reintroduce it.
2. **App artifacts were encrypted, which made apps unpublishable.** The edge is a
   separate process with no identity key: it fetches the manifest, checks it
   hashes to the claimed app id, and serves the files. It cannot decrypt
   anything. App manifests/entrypoints/assets now go through
   `put_object_public` / `put_public_file` and stay plaintext. An app is public
   output by definition, so this leaks nothing that was private.

Also: a single global `LOCK` in the runtime test module meant one panic poisoned
the mutex and turned into `PoisonError` in 7 unrelated tests, hiding real
failures. It now recovers the guard.

## Verified (real, this session)

- `cargo +1.95.0 test --workspace --exclude canopee-e2e` — 140 passed, 0 failed.
- Tray (`canopee-tray`) — 17 passed after adding `recipients: vec![]` to its two
  direct `NodeCommand::ShareObject` constructions.
- At rest, with the release binaries: stored a 34-byte canary; the plaintext
  string appears **nowhere** under the app root, and the object file holds a
  `CNP1` envelope (490 bytes). `get` returned the plaintext; `desc` reported
  `Size: 34 bytes`.
- Two real node processes over the network: A `contact-add`ed B, `share --to
  bob`, and B fetched the shared id and read `CONFIDENTIAL_PLAN_7b2e91`. The
  shared id differed from A's local original.
- Negative test: a third identity handed the *identical* ciphertext file was
  refused — `no recipient key in this envelope matches this identity`.

## Not green / open

- **mDNS is broken in this environment.** `two_nodes_pair_over_lan`,
  `two_nodes_sync_profile`, `two_nodes_periodic_sync`,
  `two_nodes_share_and_unshare_end_to_end` fail with "device did not come online
  within 20s" / "never discovered peer" on `10.42.0.66`. **Confirmed NOT a
  regression**: stashing every change and running the same test on the untouched
  baseline fails identically. Those 4 tests passed earlier in the same session,
  so the host's LAN discovery degraded mid-session. Re-prove on a healthy host.
- `publish_app_over_edge_end_to_end` is separately flaky (edge-not-ready race);
  passes on rerun.
- Pairing still dials only `qr.lan_addr` — no relay-circuit fallback, so
  first-time pairing across unrelated networks is still unsupported.
- The tray has no recipient picker, so tray sharing is owner-only (`recipients:
  vec![]`). Wiring it to contacts is follow-up work.
- Identity/device/edge key files are still plaintext on disk; only objects are
  encrypted. `identity.key` stays plaintext unless `CANOPEE_IDENTITY_PASS`.
- `cargo fmt` under 1.95 reorders imports repo-wide; I reverted the ~12 files
  that were pure formatting churn to keep the diff reviewable. Re-run fmt if you
  want it applied wholesale.

## Still unlanded from before

`DeleteObject` and `ResolveProfile` protocol/runtime/storage work exists but has
**no CLI or tray surface and no tests**. WIP files: `crates/canopee-protocol/src/lib.rs`,
`crates/canopee-node/src/lib.rs`, `crates/canopee-runtime/src/lib.rs`,
`crates/canopee-runtime/src/records.rs`, `crates/canopee-storage/src/object.rs`,
`crates/canopee-storage/src/storage.rs`.
