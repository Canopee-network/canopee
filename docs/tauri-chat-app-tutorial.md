# Tutorial: a Tauri chat app with an embedded Canopee node

This is a hands-on, step-by-step guide to a specific goal: a desktop chat
app (think: a minimal WhatsApp/Telegram) where installing the app is the
*only* setup step — no separate `canopee-node` process to start, no CLI, no
shared system-wide state. Two users install the same app and can message
each other over the Canopee network.

No code is given here — each step describes what to build, which existing
functions to hook into, and how to verify it worked before moving to the
next step. Read
[`publishing-vs-building-apps.md`](publishing-vs-building-apps.md) first —
this tutorial is building the "real `canopee-sdk` app" side of that
distinction, and Tauri turns out to be a clean way to do it (its Rust
backend *is* the bridge between a web UI and Canopee, which a plain browser
tab has no equivalent of).

## The problem, concretely

Everything in this codebase so far assumes a **separate, already-running**
`canopee-node`: the CLI's `canopee start` spawns it as its own process, and
`canopee-sdk::CanopeeClient::connect()` talks to it over a Unix socket at a
fixed path (`~/.canopee/node.sock`, from `Config::new()` in
[`crates/canopee-config/src/lib.rs`](../crates/canopee-config/src/lib.rs)).
That model is right for a dev machine running one node and several CLI
invocations against it — it's wrong for a shippable app, for two reasons:

1. **A second process the user has to know about.** "Install the chat app"
   shouldn't mean "...and also start this other background service."
2. **A single shared `~/.canopee`.** If your chat app and, say, someone's
   separately-running `canopee-cli`-driven node both point at the same
   `~/.canopee`, they'll fight over the same identity, storage directory,
   and Unix socket. Worse: if you ever want to run two *instances* of your
   own app side by side (testing, or just two accounts), they can't
   coexist with the current hardcoded path.

Both are solvable without changing the network/protocol layers at all —
you're changing *where the node lives*, not what it does.

## Step 0: orient yourself in the existing code

Read these before changing anything:

- `Runtime` in
  [`crates/canopee-runtime/src/lib.rs`](../crates/canopee-runtime/src/lib.rs)
  — notice it already exposes `identity: Arc<Identity>`, `storage:
  Arc<Storage>`, and `network: NetworkManager` as public fields, plus
  `put`/`get`/`list`/`export`/`import` methods directly. This is
  `canopee-node`'s entire job (own a `Runtime`, expose it over a socket)
  minus the socket. **You can talk to a `Runtime` directly, in-process,
  with zero IPC** — which is exactly what removes problem #1 above.
- `Config::new()` in `canopee-config` — notice it hardcodes
  `dirs::home_dir().unwrap().join(".canopee")` with no way to override the
  root. This is the actual blocker for problem #2, and the first thing
  you'll change.
- `NetworkManager::subscribe`/`publish` in
  [`crates/canopee-network/src/manager.rs`](../crates/canopee-network/src/manager.rs)
  — this is the whole mechanism `canopee chat <topic>` already
  demonstrates end to end (see
  [`testing-chat-between-peers.md`](testing-chat-between-peers.md)). Your
  chat app's message-sending is this, wired to a UI instead of a terminal
  loop — not a new protocol.
- Skim the [Tauri IPC docs](https://tauri.app/develop/calling-rust/) if
  you haven't built a Tauri app before: `#[tauri::command]` functions
  callable from JS via `invoke`, and `app.emit(...)`/listeners for
  push-style events from Rust to JS. That's the entire bridge you need —
  no new transport to design.

**Checkpoint:** explain, in one sentence, why linking `canopee-runtime`
directly (instead of spawning `canopee-node` and using
`canopee-sdk::CanopeeClient` over its socket) is the right choice for an
embedded app, given what you just read in `Runtime`.

## Step 1: make `Config` support a custom root

**Goal:** `Config` can be pointed at an app-specific data directory instead
of always resolving to `~/.canopee`.

**Where:** `Config::new()` in `crates/canopee-config/src/lib.rs`. Every
other method (`identity_path`, `storage_path`, `node_socket_path`,
`state_path`) already derives from `self.root` — you're not touching any
of those, only how `root` gets set in the first place.

**What to add:** a second constructor, e.g. `Config::with_root(path:
PathBuf)`, that skips the `dirs::home_dir()` call and uses whatever path
you pass in instead. Tauri gives you the right path to use for this via
its [`app_data_dir`](https://tauri.app/reference/config/) API (resolves to
the OS-appropriate per-app directory — e.g. `~/Library/Application
Support/<your-app>` on macOS, not the user's home directory directly).

**Design question:** should `Runtime::open()` keep calling `Config::new()`
internally, or should it take a `Config` as a parameter? Look at how
`Runtime::open()` currently constructs its own `Config` on the first line
— you'll need `Runtime` to accept a caller-supplied `Config` for your
Tauri app to actually use your new constructor. Decide whether that's a
new `Runtime::open_with_config(config: Config)`, or whether `open()`
itself should take one — and think about whether `canopee-node`'s existing
`Node::open()` (which calls `Runtime::open()` with no arguments) still
needs to keep working unchanged for the CLI's sake. You're extending an
API other code already depends on, not replacing it.

**How to verify:** without touching Tauri yet, write a small standalone
Rust test (or a `#[tokio::test]`, following the pattern in
`crates/canopee-network/tests/handshake.rs`) that opens two `Runtime`s with
two different custom roots (e.g. two temp directories) in the same
process, confirms they get two different identities (`runtime.identity()`
should differ), and that both can independently `put`/`get` objects
without colliding. This is your proof that "two instances of the app on
one machine" will actually work before you wire up any UI.

## Step 2: embed the runtime in a Tauri app, don't spawn a node

**Goal:** a Tauri app that, on startup, opens a `Runtime` in-process
(using Step 1's custom root) and keeps it alive for the app's lifetime —
no `canopee-node` process anywhere.

This is also a real security win, not just a convenience one — see
[`security-considerations.md`](security-considerations.md#local-trust-boundary-the-nodes-socket)
for why removing the shared Unix socket shrinks the local trust boundary
to "your own app's process" instead of "anything on the machine that can
reach a well-known socket path."

**Where:** Tauri's `setup` hook (in your `tauri::Builder`, run once at
startup) is where you'll call `Runtime::open()`. You'll need to store the
resulting `Runtime` somewhere the rest of your `#[tauri::command]`
functions can reach it — Tauri's
[managed state](https://tauri.app/develop/state-management/) (`app.manage(...)`,
then `State<'_, YourType>` as a command parameter) is the idiomatic way;
wrap the `Runtime` in an `Arc` (it already uses `Arc` internally for
`identity`/`storage`, and `NetworkManager` is cheaply `Clone`, so sharing
it across command invocations is safe).

**Design question:** `Runtime::open()` is `async`. Tauri's `setup` hook
can be async (there's a documented pattern for this — check the current
Tauri version's docs for the exact signature, it's changed across major
versions). Work out how you're going to run that async setup and get the
resulting `Runtime` into managed state before the frontend loads and
starts calling commands that need it — a command firing before the
runtime exists should be an explicit, handled case (return a clear error),
not a panic.

**How to verify:** add one trivial `#[tauri::command]`, e.g. `get_my_identity`,
that reads the managed `Runtime` and returns `runtime.identity().id().to_string()`.
Launch the app, call it from the frontend (a button, or just the dev
console), and confirm you get back a real `canopee://identity/...` string
— generated fresh, with no separate node process running anywhere you can
find in your process list. That's the core claim of this tutorial
("install the app, that's it") actually working.

## Step 3: exchange identities out of band

**Goal:** two users, each running their own instance of the app, end up
knowing each other's `IdentityId` (and enough network info to actually
connect) — Canopee has no phone-number-style directory, so this has to
happen outside the app itself, at least for a first version.

**Design question, not a code change yet:** how do two users tell each
other "here's my address"? A few options, each with a real tradeoff:
- **Copy-paste an identity string.** Simplest possible; add a command
  that exposes `runtime.identity()` to a "your ID" screen the user can
  copy and send to a contact through *some other channel* (a text message,
  an email — anything). Zero new infrastructure, but manual and low-trust
  (nothing stops someone claiming a fake name next to a real ID).
- **QR code.** Same data, better UX for in-person exchange — encode the
  identity string (and maybe a direct multiaddr, if you have one — see
  below) into a QR code your frontend renders, and a camera-based scanner
  on the other side. A nice-to-have on top of the copy-paste version, not
  a replacement for it.
- **A shareable link** (`canopee://identity/...` as an actual clickable
  URI, if you register a custom URI scheme with the OS) that opens your
  app directly to an "add contact" flow. More polished, more OS-specific
  plumbing (URI scheme registration differs by platform).

Whichever you build, notice this only gets you an *identity* — it doesn't
by itself get the two nodes connected. For that you still need either a
direct multiaddr to `dial` (works if you're on the same LAN, where mDNS
might already find each other automatically — see
`NetworkManager`'s `mdns` behavior in
[`canopee-network/README.md`](../crates/canopee-network/README.md)) or a
path through the DHT/bootstrap nodes, which is exactly the unsolved
piece described in
[`bootstrap-nodes-tutorial.md`](bootstrap-nodes-tutorial.md). If that
tutorial's default bootstrap list doesn't exist yet in your build, two
users on different networks won't find each other without one of them
sharing a direct multiaddr too (dialing that peer's node also seeds the
Kademlia routing table for future discovery — see that tutorial's Step 0
on why `add_address` gets called automatically on every connection).

**How to verify:** with two instances of your app running (two Tauri
windows, or two machines), exchange identity strings by whatever method
you built, then confirm one side's `dial` (against a multiaddr you get by
also exposing `runtime.network.peers()` or similar, or just by testing on
the same LAN where mDNS handles it automatically) results in `peers()`
showing the other side — same verification pattern as
[`testing-chat-between-peers.md`](testing-chat-between-peers.md)'s
same-LAN section, just triggered from your UI instead of the CLI.

## Step 4: real-time messaging over gossipsub

**Goal:** once two peers are connected, messages typed in one app's UI
show up in the other's, live.

**Where:** `NetworkManager::subscribe`/`publish`, exposed through
`Runtime::network`. This is precisely what `canopee chat <topic>` does in
`canopee-cli/src/main.rs` — read that implementation for the pattern (it's
short): subscribe once, spawn a task that forwards incoming messages
somewhere, and a separate path for sending.

**What to build**, translating that pattern to Tauri:
- **On startup** (or when a conversation is opened), call
  `runtime.network.subscribe(topic)` for whatever topic represents that
  conversation, and spawn a background task that calls `.recv()` on the
  returned `broadcast::Receiver` in a loop, forwarding each `PubSubMessage`
  to the frontend via `app.emit("message", ...)`. Your frontend listens
  for that event and appends to the visible chat.
- **A `send_message` command** that calls
  `runtime.network.publish(topic, data)` with whatever the user just
  typed.

**Design question: what's the topic?** `canopee chat` uses a
human-typed, shared topic string — fine for a public demo, wrong for a
private 1:1 chat (anyone who guesses/knows the topic string can subscribe
and read it, since gossipsub topics aren't access-controlled). A
reasonable per-conversation topic derives deterministically from both
participants' identities (e.g. a hash of the two `IdentityId`s, sorted so
both sides compute the same string regardless of who's "A" and who's "B")
— this doesn't add encryption, just stops the topic string itself from
being guessable/collide with an unrelated public topic. Real message
privacy is a separate, harder problem — see Step 6.

**How to verify:** with two connected instances (Step 3), open the same
conversation on both sides, type a message on one, and confirm it appears
on the other within the same second — the same live-delivery test
`testing-chat-between-peers.md` does with the CLI, just through your UI.

## Step 5: messages need to survive being offline

**Goal:** a message sent while the recipient is offline isn't just lost —
gossipsub only delivers to peers currently subscribed and connected, which
is very different from how WhatsApp/Telegram behave (messages wait for you
to come back online).

**Where to think about this:** [`canopee-storage`](../crates/canopee-storage/README.md)'s
`Object`/`ObjectId` model — the same primitive `app-manifest` uses to
publish files. A chat message can be stored the same way: a small signed
`Object` (`ObjectType::Blob` is fine, or add a dedicated `ObjectType` if
you want messages to be distinguishable from other blobs the way
`AppManifest` already is) containing the message text/metadata.

**The actual design problem:** publishing a message as an object gets you
tamper-proof, verifiable storage — but *how does the recipient find out a
new message object exists* if they weren't online to receive the
gossipsub `publish` in real time? A few directions, in increasing
complexity:
- **Re-send on reconnect.** Simplest: the sender keeps a local queue of
  "sent but not yet acknowledged" messages and re-publishes them (over
  gossipsub, live) whenever it detects the recipient is connected again
  (`runtime.network.peers()` shows them). Works for "recipient comes back
  online while sender is still running," fails if the sender's own app
  isn't running at the time.
- **A per-contact pointer, borrowing the app-pointer pattern.** Recall
  [`AppPointerRecord`](app-manifests.md#resolving-by-name-app-pointers) —
  a signed, mutable DHT record at a key derived from `(owner, name)`. A
  similar record per conversation, updated to point at "the latest message
  object" (or a small manifest-like list of recent message ids) every time
  you send, would let an offline recipient's app, once it comes back
  online, resolve that record and discover what it missed — without
  needing the sender to be online at the same moment. This is real,
  unbuilt work: `AppPointerRecord` as it exists today is specific to
  `(owner, name) -> manifest id`, not generalized to arbitrary use —
  you'd be designing a chat-specific equivalent, following the same shape
  (signed, DHT-published, key derived from something both sides can
  compute) rather than reusing the type directly.
- **Don't solve full offline delivery in v1.** It's legitimate to scope
  your first version to "works while both users happen to be online," note
  the limitation explicitly, and treat durable offline delivery as a
  follow-up — the same way this whole doc series scopes each tutorial to
  one concern at a time rather than demanding a complete product before
  anything ships.

**How to verify (if you build the pointer-based version):** send a message
while the recipient's app is fully closed, then start the recipient's app
and confirm it discovers and displays the missed message purely by
resolving the per-conversation record — not by the sender re-sending
anything live.

## Step 6: end-to-end message encryption

**Goal:** messages exchanged in Step 4, and stored as objects in Step 5,
are unreadable to anyone except the two participants — not the relay that
might be forwarding your connection, not another peer subscribed to (or
guessing) your conversation's topic string, not a cache node holding a
copy of a message object.

Read [`end-to-end-encryption.md`](end-to-end-encryption.md) first — it
explains what `canopee-identity` now provides (an X25519 key-agreement
primitive on `Identity`) and, at length, everything it deliberately
doesn't: encryption itself, key discovery, forward secrecy, group
messaging. This step is where this tutorial actually builds the pieces
that doc says are still missing, scoped to what a 1:1 chat needs — it
doesn't fully close every gap that doc lists (notably: no ratcheting, no
groups; see the stretch note at the end of this step).

Everything before this step is **visible to anyone who can observe it**:
gossipsub messages are signed (tampering/impersonation is detectable) but
not encrypted for a specific recipient, and stored `Object`s are
similarly signed-not-encrypted — see
[`security-considerations.md`](security-considerations.md#no-content-or-message-encryption)
for this stated as a standing concern across any app on gossipsub/objects,
not just this one. A relay in the middle of a relayed connection
(`security-considerations.md`'s Transport section) can read whatever
passes through it today, precisely because Noise is hop-by-hop, not
end-to-end — encrypting the message payload itself, before it ever
reaches `network.publish` or `Storage`, is what removes the relay (and
every other observer) from the trust picture entirely, regardless of how
many hops a message crosses.

### 6a: publish your DH public key so contacts can find it

**Where:** `Identity::dh_public_key()` in
[`canopee-identity`](../crates/canopee-identity/README.md#key-agreement-x25519)
returns this identity's X25519 public key — but nothing today lets another
peer look it up from just an `IdentityId`. You have to build that lookup;
`canopee-identity` deliberately stops at the primitive.

**What to build:** the simplest option, following the same shape as
`AppManifest` in [`canopee-storage`](../crates/canopee-storage/README.md#app-manifests-and-pointers):
a small signed `Object` — e.g. `ObjectType::Blob` containing your
`dh_public_key()` bytes plus your `IdentityId`, so it's self-describing —
that you `put_verified` locally and `announce` on the DHT at startup. A
contact resolves it via `find_providers`/`FetchObject` the same way any
other object is fetched. (A DHT record keyed by `IdentityId`, mirroring
`AppPointerRecord`, is the more "correct" long-term shape — since a
profile object's `ObjectId` changes if the DH key ever changes, and you'd
want a stable, resolvable key. Building the plain object version first is
enough to unblock the rest of this step; treat the pointer-based version
as a follow-up once you've felt the object version's limits.)

**How to verify:** with two connected instances (Step 3), have each side
fetch the other's DH-key object and confirm the bytes it gets back match
what `dh_public_key()` returns on the other instance directly (e.g. by
also exposing a debug command that prints it) — you're checking the
lookup path works before trusting it for anything cryptographic.

### 6b: derive a symmetric key, don't use `agree()`'s output directly

**Where:** `Identity::agree(&their_dh_public_key)` returns a raw
`[u8; 32]` Diffie-Hellman output — the
[`canopee-identity`](../crates/canopee-identity/README.md#key-agreement-x25519)
docs are explicit that this is not a cipher key.

**What to build:** run `agree()`'s output through a KDF (HKDF-SHA256 is
the standard choice) before using it for anything, e.g.:

```rust
let shared_secret = my_identity.agree(&their_dh_public_key);
let mut conversation_key = [0u8; 32];
hkdf::Hkdf::<sha2::Sha256>::new(None, &shared_secret)
    .expand(b"canopee-chat/conversation-key/v1", &mut conversation_key)
    .expect("32 bytes is a valid HKDF output length");
```

The domain string (`b"canopee-chat/..."`) matters for the same reason
`canopee-identity`'s own `DH_DOMAIN` constant does — it keeps this
derivation cryptographically distinct from anything else that might ever
derive a key from the same shared secret. Both sides compute the same
`conversation_key` independently (X25519 agreement is symmetric — this is
exactly what `dh_agreement_is_symmetric` in
[`crates/canopee-identity/src/identity.rs`](../crates/canopee-identity/src/identity.rs)
verifies), so there's nothing to exchange beyond the DH public keys from
6a.

**Design question:** compute this once and cache it, or derive it fresh
per session? For a first version, computing it once per contact (it's
deterministic — same two identities always produce the same
`conversation_key`) and caching it in memory is fine. The real problem
with that — one key, used forever, is exactly what forward secrecy exists
to avoid — is what the stretch note below is about; don't try to solve it
here.

### 6c: encrypt before publish, decrypt after receive

**Where:** the `send_message` command and the incoming-message handler
you built in Step 4, plus wherever Step 5 constructs a message `Object`.

**What to build:** an AEAD cipher (ChaCha20-Poly1305 or AES-256-GCM — both
have mature Rust crates) keyed with 6b's `conversation_key`, with a fresh
random nonce per message (never reuse a nonce with the same key — that's
a real, catastrophic AEAD failure mode, not a style concern). Encrypt the
plaintext message bytes immediately before calling
`runtime.network.publish(topic, ciphertext)` (or before constructing the
Step 5 message `Object`), prefixing or otherwise carrying the nonce
alongside the ciphertext (it doesn't need to be secret, just unique).
Decrypt symmetrically on receipt, before displaying anything in the UI.

At this point, revisit Step 4's topic-naming design question: a
per-conversation topic derived from both `IdentityId`s stops the topic
*string* from being guessable, and encrypting the payload stops the topic
*contents* from being readable even to someone who does guess or already
knows it — the two mitigations are independent and you want both.

**How to verify:** with two connected instances, send a message and — with
a debugger, a temporary log line, or by subscribing to the topic from a
third, unmodified `canopee chat` CLI instance — confirm what actually
crosses the network is opaque ciphertext, not the plaintext you typed.
Then confirm the real recipient still displays the decrypted plaintext
correctly. Both checks matter: the first proves privacy, the second proves
you didn't just break delivery.

### Stretch: forward secrecy and groups

6a–6c gets you real confidentiality against relays, other gossipsub
subscribers, and cache nodes — a substantial, genuine improvement over
everything before this step. It does **not** give you forward secrecy: a
single `conversation_key`, derived once and reused for every message,
means a single future key compromise exposes every past message you
encrypted with it. Signal's Double Ratchet protocol (or a simpler
per-message key-derivation scheme) exists specifically to bound that
blast radius by deriving a fresh key per message from an evolving chain,
so compromising today's key doesn't expose yesterday's messages. Building
that — along with any form of group chat, which pairwise DH doesn't cover
at all — is real, substantial follow-up work, out of scope for this
tutorial to design in full. Track it as the next thing to build once
6a–6c is working, not as something silently missing from a "done" claim.

## Summary checklist

- [ ] Step 1 — `Config` supports a custom root directory, verified with
      two independent `Runtime`s in one process
- [ ] Step 2 — a Tauri app embeds a `Runtime` directly at startup, with no
      separate `canopee-node` process, verified by a working identity
      command
- [ ] Step 3 — a way for two users to exchange identities (and connect)
      out of band, verified with two real instances finding each other
- [ ] Step 4 — live messaging over a per-conversation gossipsub topic,
      verified with real-time delivery between two instances
- [ ] Step 5 — a documented decision (and ideally an implementation) for
      what happens to messages sent while the recipient is offline
- [ ] Step 6 — DH public keys are discoverable per contact, a shared
      conversation key is derived via HKDF (not used directly from
      `agree()`), and messages are AEAD-encrypted before publish/storage —
      verified by observing ciphertext on the wire and correct plaintext
      on receipt
- [ ] Step 6 stretch — a plan, at minimum, for forward secrecy (ratcheting)
      and group messaging before calling this production-ready

By the end, two people should be able to install your app, exchange a
short identity string once (in person, over text, whatever), and message
each other for real — with the "no other setup step" claim actually true,
because there's no `canopee-node` process to separately run.

Want to build something else on this same foundation? Steps 1–3 above
(custom config root, embedded runtime, identity exchange) apply to any
Tauri app on Canopee, not just chat:

- [`tauri-multiplayer-game-tutorial.md`](tauri-multiplayer-game-tutorial.md)
  reuses them directly for a desktop multiplayer game, then covers what
  changes once your messages are game moves instead of chat text (ordering
  and delivery guarantees matter a lot more).
- [`tauri-collab-editor-tutorial.md`](tauri-collab-editor-tutorial.md)
  reuses them for a collaborative document editor, where — unlike a
  game — divergent state can't just be detected and rejected; it has to
  be automatically merged, which needs a CRDT.
- [`tauri-presence-app-tutorial.md`](tauri-presence-app-tutorial.md) is
  smaller than any of the above — worth reading first if this tutorial
  felt like a big first step.
- [`app-ideas.md`](app-ideas.md) has a longer list of app ideas beyond
  what has a tutorial yet.
