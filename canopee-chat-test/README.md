# Canopee Chat Test

A minimal desktop chat app built on [`tauri`](https://tauri.app), where two users
install the same app, exchange one contact string, and message each other end to
end encrypted over the Canopee network.

This implements the tutorial in
[`docs/tauri-chat-app-tutorial.md`](../docs/tauri-chat-app-tutorial.md):

- **Step 1** — per-app config root: the app opens its own `Runtime` via
  `canopee-runtime::Runtime::open_with_root` (which uses the new
  `canopee_config::Config::with_root`), rooted at Tauri's `app_data_dir`
  (e.g. `~/Library/Application Support/dev.canopee.chat-test` on macOS), so
  two instances on one machine never collide.
- **Step 2** — the `Runtime` is embedded **in-process** and kept in Tauri
  managed state. There is no `canopee-node` process and no Unix socket; the
  local trust boundary is just the app process.
- **Step 3** — contacts are exchanged out of band as a copy-paste string:
  `canopee://identity/<peer-id>#dh=<x25519-pubkey-hex>`. The `#dh=` fragment
  folds Step 6a's "DH key must be discoverable" into the exchanged identity
  (each side learns the other's X25519 public key from the string itself), so
  no DHT lookup is needed.
- **Step 4** — messages are published/subscribed over a gossipsub topic that
  is derived deterministically from both participants' DH keys (sorted, hashed)
  so both sides compute the same unguessable topic.
- **Step 6** — E2E encryption:
  - 6b: the conversation key is `HKDF-SHA256(domain="canopee-chat/conversation-key/v1", ikm=identity.agree(their_dh))`, never raw `agree()` output.
  - 6c: each payload is `ChaCha20-Poly1305` with a fresh random 12-byte nonce,
    transmitted as `nonce || ciphertext`.
- **Step 5 (offline delivery)** — scoped out for v1: **messages are delivered
  only while both users are online** (gossipsub). No per-conversation pointer
  record or re-send queue yet.

Forward secrecy and group chat are likewise out of scope (tutorial's stretch
note): one deterministic conversation key, reused for every message.

## Run it

```bash
cd canopee-chat-test
cargo tauri dev
```

Two instances on the same LAN auto-discover each other via mDNS, so running the
app twice (e.g. two machines, or two app binaries) is enough to test. If peers
don't connect over mDNS, dial manually from the JS console:

```js
dial("/ip4/<address>/tcp/<port>/p2p/<peer-id>")
```

(Use the details sent to you by the other side, or copy your own from the app's
"network details" panel.)

## Test instructions (two instances)

1. Launch app **A** and app **B** (same app binary twice is fine; each instance
   has its own data dir).
2. In **B**, copy its contact string (top of the window) and paste it into
   **A** (click Add contact, name it `bob`).
3. Do the same in **B** for **A**'s string, name it `alice`.
4. Select the contact on each side and type — messages appear on the other side
   within a second or two.

If delivery seems stuck, confirm both apps show `1 peer(s) connected` in the
header (mDNS/discovery succeeded).

## What's where

- `src-tauri/src/lib.rs` — Tauri backend: setup (embedded `Runtime`), managed
  state, commands (`get_my_identity`, `get_self`, `add_peer`, `send_message`,
  `get_peers`, `dial`, `list_conversations`), and the per-conversation receive
  loop that decrypts and `emit`s `chat-message`.
- `src-tauri/src/chat.rs` — pure, unit-tested core: contact-string parsing,
  topic derivation, HKDF key derivation, ChaCha20-Poly1305 encrypt/decrypt.
- `src-tauri/src/main.rs` — thin binary entry point.
- `src-tauri/tauri.conf.json` + `capabilities/default.json` — window, static
  frontend (`dist/`), and permissions.
- `dist/index.html`, `dist/app.js` — the UI (dependency-free, no build step).

## Tests

```bash
cd canopee-chat-test
cargo test
```

The crypto core (contact parsing, symmetric key derivation, AEAD round-trip,
wrong-key/tamper rejection) is covered without needing a GUI or a network.
Step 1's two-isolated-runtimes check lives in
`crates/canopee-runtime/src/lib.rs` (`with_root_gives_isolated_identities_and_storage`).

## Architecture notes

- Uses `canopee-runtime` directly (in-process), **not** `canopee-sdk`'s socket
  client. The tutorial's Step 0 checkpoint explains why: an embedded app has no
  second process and no shared `~/.canopee`, and `Runtime` already exposes
  `identity`/`storage`/`network` as public fields — the socket is purely the
  CLI's IPC, not a requirement.
- The app adds the workspace crates as path dependencies and is a standalone
  Cargo workspace, so its `Cargo.lock` (including Tauri's dependency tree)
  doesn't touch the repo's.