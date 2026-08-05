# Tutorial: a peer-to-peer collaborative document editor

This is a hands-on, step-by-step guide to the most technically demanding
app in this tutorial series: a shared text document that multiple people
can edit at once, live, with no server — think a tiny P2P alternative to
Google Docs. It's meaningfully harder than
[`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md) or
[`tauri-multiplayer-game-tutorial.md`](tauri-multiplayer-game-tutorial.md)
for one specific reason: both of those tutorials get to *reject* a
divergent state (end the match, or just accept that a chat message
arrived a moment late) — a document editor cannot. Two people typing in
different parts of the same paragraph at the same time must converge to
*one* correct combined result, automatically, every time. That's a genuinely
different problem, and this tutorial treats it as one rather than
pretending it's a small variation on the game tutorial's pattern.

No code is given here — each step describes what to build, which existing
functions to hook into, and how to verify it worked before moving to the
next step. **Read [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md)
first, in full** for Steps 1–3 (custom config root, embedding a `Runtime`
in Tauri, exchanging identities), which this tutorial reuses without
repeating. Reading
[`tauri-multiplayer-game-tutorial.md`](tauri-multiplayer-game-tutorial.md)'s
Step 4-5 first is also worth doing, purely as a contrast — this tutorial
explains, in Step 0, exactly why that game's approach (detect divergence,
abort) doesn't work here and what has to happen instead.

## The problem, concretely

Two people, Alice and Bob, are both looking at the same document. Both
type at the same moment, in different places. Over an unordered,
best-effort transport (gossipsub — see
`NetworkManager::publish`/`subscribe` in
[`crates/canopee-network/src/manager.rs`](../crates/canopee-network/src/manager.rs)),
Alice's edit and Bob's edit arrive at each side in whatever order the
network happens to deliver them, possibly a different order on Alice's
screen than on Bob's.

The multiplayer game tutorial's answer to "state might diverge" was:
compute a checksum, detect a mismatch, and stop the match rather than try
to reconcile. **That answer is completely wrong for a document editor.**
You cannot pause someone's typing and show them "these two clients
disagree, please restart" every time two edits land close together in
time — that would make the editor unusable within the first minute of
real use. The correct answer is: **structure the data so that applying
the same set of edits in *any* order always produces the *same* final
document**, so there's nothing to detect or abort — divergence is
prevented by construction, not caught after the fact.

This property — same result regardless of the order operations are
applied in — is exactly what a **CRDT** (Conflict-free Replicated Data
Type) provides. This tutorial doesn't invent CRDT theory from scratch;
it's a genuinely deep area with existing, well-studied algorithms. What it
does is walk through *integrating* one with what Canopee already gives
you (identity, gossipsub, object storage), and being explicit about the
decisions that are yours to make versus the ones a CRDT library makes for
you.

## Step 0: pick your CRDT approach — don't build your own

**Goal:** before writing any networking code, decide what data structure
your document actually is under the hood, and confirm you're using an
existing, tested implementation rather than inventing your own merge
logic.

**Why this matters enough to be its own step:** collaborative text editing
CRDTs (the family usually called RGA, Fugue, or the specific algorithm
behind Automerge/Yjs) are subtle — getting them right from scratch is a
research-level undertaking, not a weekend project, and getting them
*almost* right produces bugs that look like "sometimes, rarely, a
character goes missing or duplicates," which are exactly the kind of bug
that's brutal to track down after the fact. Use an existing Rust crate:

- **[`yrs`](https://github.com/y-crdt/y-crdt)** — the Rust port of Yjs, a
  mature, widely-used CRDT library specifically designed for collaborative
  text/rich documents. Probably the most direct fit for "a text document."
- **[`automerge`](https://github.com/automerge/automerge-rs)** — another
  mature, well-documented option, slightly more general-purpose (works
  for arbitrary JSON-like structures, not just text), with good Rust-native
  ergonomics.

Pick one (this tutorial's later steps refer to "the CRDT library" generically
— the integration shape is the same regardless of which one you pick) and
work through its own quickstart/tutorial *outside* of Canopee entirely
first — get comfortable with its API for "create a document," "apply a
local edit," "produce a binary update to send to a peer," and "apply a
remote peer's update," in a plain standalone Rust program with no
networking at all. This is worth doing before touching Canopee, because it
isolates "am I using the CRDT library correctly" from "is my networking
code correct" — two separate things you don't want to debug at once.

**Checkpoint:** in a standalone test program (no Tauri, no networking),
simulate two "replicas" of the same CRDT document, apply a different edit
to each, exchange updates between them in both possible orders, and
confirm both replicas converge to the identical final document regardless
of which order you fed the updates in. This is the property you're
relying on for everything that follows — prove it to yourself in isolation
first.

## Step 1–2: reuse the chat tutorial's foundation

Do [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md)'s Steps 1–3
exactly as written (custom `Config` root, embedding a `Runtime` in Tauri's
managed state, exchanging identities and getting two instances connected)
— no changes needed here. This tutorial picks up assuming that foundation
already works.

## Step 3: a shared document identified by an object, not a name

**Goal:** a way for a document to have a stable identity that multiple
people can independently open, distinct from `AppPointerRecord`'s
`(owner, name)` pattern — a shared document isn't "owned" by one person
the way a published app is.

**Where to think about this:** `Object`/`ObjectId` in
[`crates/canopee-storage`](../crates/canopee-storage/README.md) still
apply here, but differently than in the app-manifest flow. Consider
publishing an initial, empty CRDT document as a signed `Object` (any
`ObjectType` works, since — like `AppPointer` — nothing about a "shared
document" is a first-class `ObjectType` today; you're using the existing
generic object mechanism, not a new dedicated one), and using *that
object's `ObjectId`* as the document's shareable identity — the same way
a manifest id is what you share for an app. Whoever creates the document
shares this id with their collaborators out of band (same identity/address
exchange problem as every other tutorial in this series — see
[`bootstrap-nodes-tutorial.md`](bootstrap-nodes-tutorial.md) if discovery
beyond direct dialing matters to you).

**Design question:** should the *document id* also be the gossipsub
*topic* collaborators subscribe to for live edits, or should the topic be
derived from the document id (e.g. hashed together with a fixed prefix, the
same pattern the chat tutorial uses to derive a private conversation
topic)? Using the raw id directly is simpler; deriving a topic is a small
extra step that avoids ambiguity if you ever reuse object ids for
something else in the same app. Either is defensible — just be consistent
and document which you picked.

**How to verify:** create a document (publish the initial empty CRDT
state as an object), share its id with a second instance, and confirm the
second instance can `get`/`fetch_object` it and load an empty document —
no live editing yet, just "can I open a document I was given the id for."

## Step 4: live edits over gossipsub

**Goal:** as each person types, their local CRDT library produces a small
binary update representing that edit; broadcast it, and apply every
update you receive from anyone else — that's the entire live-sync
mechanism, because the CRDT library is what guarantees the result
converges regardless of arrival order.

**Where:** `NetworkManager::publish`/`subscribe` on the document's topic
(from Step 3), exactly the mechanism every other tutorial in this series
uses for live traffic — the only thing that's different here is *what*
you're sending: your CRDT library's own binary update format, not chat
text or a game move.

**What to build**, translating that to Tauri commands and events:
- Whenever the local editor UI produces a change (the user types), call
  your CRDT library's "apply local edit, produce an update" function, then
  `publish` that update's bytes on the document's topic.
- Subscribe to the topic and, for every update you receive from another
  peer, apply it to your local CRDT document via the library's "apply
  remote update" function, then re-render whatever the library's current
  merged text content is into your editor UI.

**Design question — this is the one place your own new code has real
responsibility, even with a CRDT library doing the merge:** how do you
reconcile the CRDT library's merged output with what's currently sitting
in a live text-editing UI component (a `<textarea>`, or a rich editor
component) without fighting the user's own cursor position and in-flight
keystrokes? This is a genuinely fiddly UI problem independent of Canopee
or the CRDT math — most CRDT text libraries' own documentation/examples
cover exactly this integration pattern (applying remote updates without
clobbering local edit state); lean on your chosen library's own examples
here rather than solving it from scratch.

**How to verify:** with two connected instances viewing the same document
id, type in one and confirm the text appears in the other within roughly a
second. Then do the actual test that matters: **type simultaneously in
both**, in different parts of the document, and confirm both sides
converge to showing the *same* final merged text — not just "both users'
edits eventually show up somewhere," but genuinely identical content on
both sides, character for character. This is the test the game
tutorial's Step 5 (state-hash mismatch detection) has no equivalent for —
here, there should be nothing to detect, because divergence shouldn't be
possible by construction.

## Step 5: persistence and late joiners

**Goal:** someone who opens the document after edits have already
happened — or reopens the app after closing it — sees the current,
merged content, not just whatever gossipsub messages happen to still be
in flight (which, per every other tutorial in this series, is nothing —
gossipsub has no message history).

**Where:** back to `Storage`/`Object` — periodically (e.g. after a short
debounce whenever edits stop for a moment, not on every single keystroke)
serialize your CRDT document's full current state via your library's own
"export full state" function, and `put_object` it as a new `Object`. You
now have a choice, structurally identical to the chat tutorial's Step 5
problem ("how does someone who was offline discover what they missed"):

- **Simplest:** whoever creates the document publishes an
  `AppPointerRecord`-style pointer (same pattern, different payload — a
  signed `(document-creator-identity, document-name)` → latest-full-state
  `ObjectId` mapping) that a late joiner resolves to fetch the current
  state before subscribing to live updates going forward. This mirrors
  [`app-manifests.md`](app-manifests.md#resolving-by-name-app-pointers)'s
  pointer pattern directly, just pointing at CRDT state instead of an
  `AppManifest`.
- **More robust, more work:** every participant (not just the creator)
  periodically publishes their own snapshot pointer, so the document
  doesn't have a single point of failure if its original creator goes
  offline — directly analogous to
  [`p2p-app-caching-tutorial.md`](p2p-app-caching-tutorial.md)'s whole
  premise (any peer who has a full copy can serve it, not just the
  original publisher).

**How to verify:** with the document already containing some edited
content, start a *third* instance that has never seen this document
before, give it only the document id (or the creator's identity + a name,
if you built the pointer approach), and confirm it loads the current
merged content correctly — then have it make an edit and confirm that edit
also propagates live to the other two instances, proving it's a full
participant, not just a read-only viewer.

## Step 6 (stretch): who can edit, and privacy

Two gaps worth naming honestly, matching
[`security-considerations.md`](security-considerations.md)'s pattern of
calling out what's *not* solved rather than implying it is:

- **No access control.** Anyone who learns the document's id (or resolves
  its pointer) can subscribe to its topic and start publishing edits —
  there's no concept of "invited collaborators only" anywhere in this
  design. For a small-group, trusted-by-default tool this may be
  perfectly fine (the same trust model
  [`tauri-multiplayer-game-tutorial.md`](tauri-multiplayer-game-tutorial.md#step-6-stretch-cheating-and-trust)
  assumes for its cooperative game setting); for anything wider, you'd
  need an explicit allowlist of identities permitted to edit, checked
  before applying an incoming update — genuinely new design work, not
  covered here.
- **No encryption.** Same gap as everywhere else in this series — see
  [`security-considerations.md`](security-considerations.md#no-content-or-message-encryption).
  Document content, both live edits and stored snapshots, is visible to
  anyone who can observe the gossipsub traffic or fetch the stored
  objects. Don't market this as a private document editor without solving
  this first.

## Summary checklist

- [ ] Step 0 — picked an existing, tested CRDT library (not a
      hand-rolled one) and proved convergence in a standalone test before
      touching Canopee at all
- [ ] Steps 1–2 — reused from `tauri-chat-app-tutorial.md`: custom config
      root, embedded runtime, identity exchange
- [ ] Step 3 — a document has a stable, shareable id backed by a real
      `Object`, verified by a second instance opening an empty document
      by id
- [ ] Step 4 — live edits sync over gossipsub, verified specifically by
      simultaneous edits on both sides converging to identical content
- [ ] Step 5 — persistence and late-joiner support via periodic snapshots
      and a resolvable pointer, verified with a fresh third instance
      loading existing content correctly
- [ ] Step 6 (stretch) — explicit, honest decisions about access control
      and encryption before calling this ready for anything beyond a
      trusted small group

By the end, multiple people should be able to edit the same document at
once, from separate installs with no server, and see the same correct
result — not because conflicts were caught and resolved, but because the
underlying CRDT made conflicting states impossible to reach in the first
place.
