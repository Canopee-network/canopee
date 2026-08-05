# Tutorial: a Tauri multiplayer game with an embedded Canopee node

This is a hands-on, step-by-step guide to a specific goal: a desktop
multiplayer game (think: a simple lockstep game — a card game, a board
game, a small real-time game with a handful of players) where installing
the app is the only setup step, and players find and play against each
other over the Canopee network with no game server to run or pay for.

No code is given here — each step describes what to build, which existing
functions to hook into, and how to verify it worked before moving to the
next step. **Read [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md)
first, in full** — this tutorial reuses its Steps 1–3 (custom `Config`
root, embedding a `Runtime` in Tauri, exchanging identities out of band) 
without repeating them, and picks up from "two Tauri app instances can find
each other and are connected." Everything from Step 4 onward here is new
and specific to *game* traffic rather than chat.

## The problem, concretely

A multiplayer game and a chat app share almost their entire foundation —
identity, discovery, embedding the runtime, transport — and diverge on
exactly one thing: **what the messages mean, and what happens when they
don't all arrive in order.**

Chat tolerates gaps and reordering fine — a human reading a chat log
barely notices if two messages arrive a few milliseconds out of order.
A game does not: if Player A's "move piece to C4" and Player B's "move
piece to D4" arrive in a different order on two different players'
screens, the two players are now looking at two different game states,
and nothing forces them back into agreement. This is the actual problem
this tutorial solves — not "how do I send data between two peers" (that
part is identical to chat), but "how do I make sure everyone agrees on
what happened, given a transport with no ordering or delivery guarantees."

Concretely, gossipsub (what `NetworkManager::publish`/`subscribe` uses)
gives you **no** ordering guarantee across messages, **no** delivery
guarantee (a peer who's briefly disconnected simply misses a message,
with no retry), and **no** built-in concept of "everyone has definitely
seen this." All three matter enormously for a game and barely at all for
chat.

## Step 0: orient yourself, and decide your game's shape first

Before writing anything, answer these — they determine which
architecture you need in Step 4, and are impossible to answer generically:

- **How many players, and how synchronous does it need to be?** A 2-player
  turn-based board game (chess, a card game) has very different
  requirements from a 4-player real-time action game. This tutorial is
  written primarily for the turn-based/lockstep case — it's tractable
  without a dedicated networking library, and honestly represents the
  ceiling of what's reasonable to build on raw gossipsub. If you want
  real-time continuous movement with interpolation/prediction, treat this
  tutorial as a starting point, not a complete answer — that's a
  substantially harder networking problem or a level.
- **Does every player need to trust every other player's client?**
  Without a server, every player's own game state is only as trustworthy
  as their own client — there's no neutral referee. This tutorial assumes
  a *cooperative-trust* setting (friends playing together, not a
  competitive ranked ladder where cheating has real stakes) — see
  [Step 6](#step-6-stretch-cheating-and-trust) for why "no dedicated
  server" and "cheat-proof" are in direct tension.
- Re-read `NetworkManager::subscribe`/`publish` in
  [`crates/canopee-network/src/manager.rs`](../crates/canopee-network/src/manager.rs)
  and `PubSubMessage` in
  [`crates/canopee-protocol/src/lib.rs`](../crates/canopee-protocol/src/lib.rs)
  — notice `PubSubMessage` carries only `topic`, `source` (the sender's
  peer id), and raw `data`. No sequence number, no timestamp, no
  acknowledgment. Anything resembling ordering or reliability is
  something *you* build on top, not something the transport gives you.

**Checkpoint:** write down, in a sentence each: how many players your game
supports, whether it's turn-based or real-time, and what happens if two
players briefly disagree about game state (does the game have a way to
detect and recover from this, or does it assume it never happens?).
You'll refer back to this when making Step 4's design decisions.

## Step 1–3: reuse the chat tutorial's foundation

Do [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md)'s Steps 1–3
exactly as written, with no changes:

- **Step 1** — `Config::with_root(...)` so your game app has its own data
  directory, independent of any other Canopee app on the same machine.
- **Step 2** — embed a `Runtime` in your Tauri app's `setup` hook, managed
  state, no separate `canopee-node` process.
- **Step 3** — a way for two players to exchange identities (copy-paste,
  QR code, whatever you built) and get their nodes connected, either
  directly (`dial`) or via bootstrap/DHT discovery (see
  [`bootstrap-nodes-tutorial.md`](bootstrap-nodes-tutorial.md) if that's
  not solved in your build yet).

**How to verify:** identical to the chat tutorial's Step 3 verification —
two running instances, `peers()` shows the other side. Don't proceed until
this works; everything below assumes it does.

## Step 4: a lockstep match protocol

**Goal:** two (or more) connected players agree to start a match, and every
move either player makes is seen by both, in the same order, before the
game advances.

**Where:** you're designing new message types and new Tauri commands here
— there's no existing "match" or "move" concept anywhere in Canopee to
build on; `NetworkManager::publish`/`subscribe` is your only primitive.
Design your own small set of message types (however you serialize them —
`serde`/`bincode`, matching the convention every other wire type in this
codebase already uses, e.g.
[`canopee-protocol/src/lib.rs`](../crates/canopee-protocol/src/lib.rs)),
something like: an invite/accept pair to start a match, and a move message
carrying whatever your game's move representation is.

**The core design problem — lockstep ordering:** since gossipsub gives you
no ordering guarantee, you need your *own* concept of turn order that both
sides enforce independently, rather than trusting arrival order. The
standard approach for a turn-based game:

- Assign each match a sequence number, starting at 0, incremented by
  whoever's turn it is *before* they publish their move.
- Every move message carries its sequence number.
- A client that receives a move whose sequence number isn't exactly "my
  current expected sequence number" holds it (if it's ahead — the sender's
  message arrived, but an earlier one from the *other* player hasn't yet)
  or discards it as a duplicate (if it's behind). This is the mechanism
  that turns "gossipsub might reorder things" into "the game state is
  still deterministic," even though the transport doesn't guarantee
  ordering — you're rebuilding ordering at the application layer, on top
  of an unordered transport, which is the standard technique any lockstep
  networked game uses (this predates P2P entirely — it's how early
  real-time strategy games like the original Age of Empires synchronized
  players over plain UDP).
- Only advance your local game state (and re-render the UI) once a move
  has been slotted into its correct sequence position — never optimistically
  apply a move you received out of order.

**Design question:** what happens when a message is not just reordered
but genuinely lost (the sender was briefly disconnected)? Gossipsub gives
you no retry. Decide: does the sender re-publish periodically until it
sees the *other* player's next move (implying receipt), or do you
build an explicit acknowledgment message (a third message type: "I got
move #4") and have the sender retry until it sees one? The
acknowledgment approach is more robust and not much more code — strongly
consider building it rather than hoping gossipsub's own retry behavior
(which exists but isn't tuned for this) is enough.

**How to verify:** with two connected instances, play a full match end to
end — moves alternating, both screens staying in sync. Then simulate the
failure mode directly: briefly kill the network connection between the
two instances (unplug wifi, or add a deliberate `--peer`-independent
disconnect if you're testing locally) mid-match, reconnect, and confirm
the game either recovers cleanly (via your retry/ack mechanism) or fails
in an obvious, handled way (a "connection lost" screen) — never silently
diverging into two different game states with no indication anything went
wrong.

## Step 5: detecting divergence

**Goal:** if the lockstep mechanism from Step 4 has a bug (or a message
somehow slips through corrupted, or a client has a genuine software bug),
catch it — don't let two players silently believe different things
happened in the same match.

**Where:** this is a new, small addition on top of Step 4's move
messages, not a separate system. Compute a simple checksum/hash of your
full game state after applying each move (a hash of your game's own state
struct — how you serialize it for hashing is entirely up to your game's
data model) and include it in every move message alongside the move
itself.

**What to build:** when a player receives a move, apply it locally, then
compare *your own* freshly computed state hash against the hash the sender
included in that same message. If they match, you and the sender agree.
If they don't, something has already gone wrong — and this is the moment
to surface it, not to let the game silently continue diverging further
with every subsequent move.

**Design question:** what do you actually do on a mismatch? For a
cooperative/friendly game, the honest, simple answer is: show both players
an explicit "these two clients disagree about game state" message and end
the match, rather than attempting automatic reconciliation (which requires
picking whose state is "right," and you have no authoritative referee to
make that call in a pure P2P design — see Step 6). Silent failure is the
one thing you should design out entirely, even if the "clean" fix
(reconciliation) is out of scope.

**How to verify:** deliberately introduce a state-hash mismatch — e.g. a
debug build flag that skips applying every 5th move on one instance only
— and confirm your mismatch detection fires and the game surfaces it
clearly, rather than the two clients continuing to play out of sync with
neither player any wiser.

## Step 6 (stretch): cheating and trust

Worth naming honestly, the way
[`security-considerations.md`](security-considerations.md) does for other
gaps: **without a dedicated authoritative server, "prevent a player from
cheating" and "no dedicated server" are in direct tension.** A malicious
client can:

- Claim moves it didn't actually make according to the game's rules (if
  your implementation doesn't independently validate every incoming move
  against the game's actual legal-move logic before applying it — this is
  non-negotiable even for a friendly game; always validate, never trust
  the sender's claim that a move is legal).
- Send a move, observe the resulting game state, then claim a *different*
  state hash than what it actually computed, specifically to trigger
  Step 5's divergence detection and abort a match it's about to lose.
  There's no way to fully prevent this in a pure two-peer design — it's a
  structural consequence of neither side being a neutral referee.

For a genuinely trust-sensitive competitive setting (ranked play, real
stakes), the standard fix is a third, mutually-trusted party — either a
dedicated server (which defeats the "no server" premise this tutorial is
built around) or, in some P2P game designs, a rotating "host" role among
players with some reputation/consensus mechanism for disputing a host's
claims. Neither is designed here; treat this as the honest ceiling of what
a pure-P2P, no-infrastructure game can guarantee, and scope your own game
to a setting (friends playing together) where that ceiling is acceptable.

## Summary checklist

- [ ] Step 0 — decided your game's shape (player count, turn-based vs.
      real-time, trust model) before writing any networking code
- [ ] Steps 1–3 — reused from `tauri-chat-app-tutorial.md`: custom config
      root, embedded runtime, identity exchange
- [ ] Step 4 — a lockstep move protocol with application-level sequencing
      and a retry/acknowledgment mechanism, verified against both a clean
      match and a simulated disconnect
- [ ] Step 5 — state-hash checking on every move, verified by deliberately
      injecting a mismatch and confirming it's caught, not silently ignored
- [ ] Step 6 (stretch) — an explicit, honest decision about what level of
      cheat-resistance your game needs, and whether a pure P2P design can
      actually provide it

By the end, two players should be able to install your app, exchange an
identity once, and play a full match with confidence that if anything ever
goes wrong — a dropped connection, a corrupted message, a bug — it's
caught and surfaced, not silently producing two different "truths" about
the same game.
