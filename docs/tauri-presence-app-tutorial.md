# Tutorial: a presence app — who's online right now

This is a hands-on, step-by-step guide to the simplest possible real
`canopee-sdk` app: a small desktop app that shows which of your contacts
are currently online, using only `peers()` and a gossipsub heartbeat. If
[`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md) felt like a big
first step, start here instead — this tutorial is deliberately smaller and
is a good warm-up before either that one or
[`tauri-multiplayer-game-tutorial.md`](tauri-multiplayer-game-tutorial.md).

No code is given here — each step describes what to build, which existing
functions to hook into, and how to verify it worked before moving to the
next step. Read
[`publishing-vs-building-apps.md`](publishing-vs-building-apps.md) first
if you haven't — this tutorial builds the "real `canopee-sdk` app" side of
that distinction, same as the chat and game tutorials, just with a much
smaller surface.

## The problem, concretely

"Who's online" sounds trivial, but it exposes a real distinction worth
understanding early: **being *connected* to a peer and knowing their
*identity* are two different things**, and Canopee's current plumbing only
gives you one of them automatically.

- `NetworkManager::peers()` (via `Runtime::network` or
  `canopee-sdk::CanopeeClient::peers()`) tells you who you're currently
  connected to, as libp2p `PeerId`s — this is real, live connection state.
- And `Peer` in
  [`crates/canopee-network/src/peer.rs`](../crates/canopee-network/src/peer.rs)
  has an `identity: Option<IdentityId>` field that `canopee-network` fills
  in for you — but only after you ask it to.

Here's the subtlety this tutorial rests on: a peer's libp2p `PeerId` is
its **device** id (each device mints its own key; see
[`multi-device-identity.md`](multi-device-identity.md)), while an
`IdentityId` is the *account* that device carries —
`canopee://identity/<account-key-peer-id>`. For a device paired onto an
existing identity these are different strings, so you can **not** just
format `peer.peer_id` into an identity string the way you'd have before
device-key separation. Instead, the runtime gives you
`Runtime::enrich_peers()`: it resolves each connected peer's identity
through the `device:<peer-id> → identity` registry (falling back to the
legacy identity-bound format only for pre-device-key peers), enriches the
peer with the resolved username/display name too, and stores the result in
the `Peer` structs. Call it in your polling loop before mapping peers, and
read `peer.identity` from the result.

## Step 0: orient yourself in the existing code

Read these before changing anything:

- `NetworkManager::peers()` / `Peer` in
  [`crates/canopee-network/src/manager.rs`](../crates/canopee-network/src/manager.rs)
  and `peer.rs` — this is your entire "who am I connected to" data source.
- `NetworkManager::publish`/`subscribe` — same mechanism
  [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md) uses for
  messages; here you're publishing small heartbeat pings instead of chat
  text.
- `Runtime::enrich_peers()` in
  [`crates/canopee-runtime/src/lib.rs`](../crates/canopee-runtime/src/lib.rs) —
  the registry-backed peer→identity resolution you'll call before display,
  instead of deriving the identity string from `peer_id` by hand.
- If you haven't done
  [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md)'s Steps 1–2
  yet (custom `Config` root, embedding a `Runtime` in a Tauri app's
  managed state), do those first — they're identical here and won't be
  repeated. This tutorial picks up assuming you already have an embedded
  `Runtime` reachable from `#[tauri::command]` functions.

**Checkpoint:** explain, in one sentence, why `peer.peer_id` alone is no
longer enough to show a contact's identity string, and what
`enrich_peers()` gives you instead.

## Step 1: show directly-connected peers

**Goal:** a UI listing every peer your node is currently connected to, by
identity string, with no heartbeat or contact list yet — the simplest
possible "who's around."

**Where:** one new `#[tauri::command]`, e.g. `list_connected_peers`, that
calls `runtime.enrich_peers().await` first (Step 0's registry-backed
resolution), then `runtime.network.peers().await` and maps each `Peer` to
its `peer.identity` string — falling back to
`format!("canopee://identity/{}", peer.peer_id)` only for legacy
pre-device-key peers, exactly as `enrich_peers` itself does. Displaying
`peer.identity` is consistent with what `canopee identity` prints.

**Design question:** should this be a one-shot call your frontend polls
periodically, or should you push updates to the frontend proactively
(e.g. `app.emit(...)` whenever `peers()`'s result changes)? Polling is
simpler to build first — start there, and treat pushing changes as a
later refinement once you have something working end to end. There's no
existing "peer connected/disconnected" event stream exposed by
`NetworkManager` today; building one would mean watching for
`SwarmEvent::ConnectionEstablished`/`ConnectionClosed` inside
`canopee-network` itself and exposing a new subscription — real, but
optional work, not required for this tutorial's goal.

**How to verify:** with two Tauri instances connected (per the chat
tutorial's Step 3), open your presence UI on both sides and confirm each
shows the other's identity string. Disconnect one (kill the process or
network) and confirm — after your polling interval — the other side's
list updates to no longer show it. This alone is already a real, working
presence indicator, just limited to peers you're directly connected to.

## Step 2: presence beyond direct connections

**Goal:** know whether a specific *contact* (someone you've added, not
just whoever you happen to be directly connected to right now) is online
anywhere on the network — not only when directly connected to them.

**The core problem:** `peers()` only reflects direct libp2p connections.
If Alice and Bob are both online but connected to the network via
different paths (e.g. through different relays, or simply not dialed to
each other), neither's `peers()` list shows the other, even though both
are genuinely reachable. You need a way to ask "is this specific identity
online" that doesn't require already being connected to them.

**Where:** gossipsub, the same mechanism chat uses for messages. Design a
small heartbeat: each contact you want to track presence for gets its own
topic (deterministically derived from their identity, or from both
participants' identities the same way
[`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md#step-4-real-time-messaging-over-gossipsub)'s
Step 4 derives a private conversation topic — reuse that exact idea
here). Both sides `subscribe` to that topic and periodically `publish` a
small "I'm here" ping (a timestamp is enough payload).

**What to build:**
- On startup (or when a contact is added), subscribe to that contact's
  presence topic and spawn a background task that publishes a ping on it
  every few seconds — matching roughly the cadence gossipsub's own
  heartbeat already runs at internally (1 second by default; pinging
  every 3-5 seconds is a reasonable, not-too-chatty choice) is a
  reasonable, non-spammy interval.
- Track, per contact, the timestamp of the last ping you *received* from
  them. In your UI, show them as "online" if that timestamp is recent
  (within, say, 2-3x your ping interval — enough slack to tolerate one
  missed heartbeat without immediately flickering to "offline") and
  "offline" otherwise.

**Design question:** what happens the moment you subscribe to a contact's
topic but they're not currently running their app at all? Nothing —
you'll simply never receive a ping, and correctly show them as offline.
This is the expected, correct behavior, not a bug — worth explicitly
testing (see below) so you're confident "no pings received" and "actively
told me they're offline" look the same in your UI, since gossipsub has no
"goodbye" message by default (you could add one — a deliberate
"I'm going offline" ping published right before your app closes — as a
nice-to-have that makes the UI feel more immediate, but the timeout-based
approach must work correctly on its own regardless, since a crashed app
never gets to send a goodbye).

**How to verify:** with two connected instances, confirm each shows the
other as "online" within a few seconds of both apps starting. Then quit
one app entirely (not just disconnect the network — actually close the
process) and confirm the other side transitions to "offline" once its
timeout window elapses, without you having done anything explicit on the
quitting side. Then test the reverse: start the second app fresh, with
the first *not yet running*, and confirm it correctly shows the contact as
offline immediately (no false "online" from a stale prior state).

## Step 3: a persistent contact list

**Goal:** contacts you've added stay in your list across app restarts,
not just for the current session.

**Where:** this is ordinary local persistence, not a Canopee-specific
problem — store a small list of `(name-you-gave-them, their identity
string)` pairs in your app's own data directory (the same directory Step
1 of the chat tutorial pointed `Config::with_root` at is a reasonable
place, though it doesn't have to be — this data isn't a Canopee `Object`,
it's local app state, so a plain JSON file or a small embedded database is
fine).

**Design question:** should you subscribe to *every* contact's presence
topic immediately on app startup, or lazily (e.g. only while your contact
list screen is actually open)? Subscribing to everyone immediately gives
you accurate presence the moment the app opens, at the cost of running a
background task per contact for the app's entire lifetime — for a small
contact list (tens of people) this is completely fine; if you're
imagining hundreds of contacts, lazy subscription (only for contacts
currently visible) becomes worth considering. Don't over-engineer this
prematurely — start with "subscribe to everyone on startup" and only
revisit if you actually have a reason to.

**How to verify:** add a contact, quit the app entirely, relaunch it, and
confirm the contact is still in your list and presence detection resumes
(you don't have to re-add them or re-exchange identities).

## Summary checklist

- [ ] Step 0 — understood that a peer's libp2p `PeerId` is its device id,
      not its identity, and used `enrich_peers()` to resolve
      `Peer::identity`
- [ ] Step 1 — a UI showing directly-connected peers by identity string,
      verified with two connected instances
- [ ] Step 2 — per-contact gossipsub heartbeats giving accurate
      online/offline status even without a direct connection, verified by
      both a clean online transition and a real process-quit going
      correctly to "offline"
- [ ] Step 3 — a persistent contact list surviving app restarts

By the end, you'll have the smallest complete `canopee-sdk` app in this
tutorial series — a good foundation to extend into anything that needs
"is this person around right now" as a building block (a chat app's
online indicator, a game's "invite a friend who's currently online," or
its own standalone thing).
