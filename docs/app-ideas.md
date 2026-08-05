# Roadmap: what other apps are possible, and which deserve a tutorial

This is a gap-analysis-style list, not a tutorial — see
[`roadmap-hosting-replacement.md`](roadmap-hosting-replacement.md) for the
sibling doc this one is modeled on. It exists to answer: given what
Canopee actually provides (identity, signed content-addressed objects,
gossipsub pub/sub, a DHT for discovery and mutable pointers), what else is
worth building, and which ideas are chosen specifically because they
exercise a *different* combination of primitives than what's already been
written up?

Every app idea here is placed in one of three buckets, same structure as
the hosting-replacement roadmap:

1. **Has a tutorial already.**
2. **Deserves a tutorial — scoped and reasoned about here, not yet
   written.**
3. **Interesting, but not yet well-scoped enough to commit to a tutorial**
   — included so the idea isn't lost, with an honest note on what's
   missing before it could become one.

## 1. Has a tutorial

| App | Tutorial | What it exercises |
|---|---|---|
| Static site / SPA hosting | [`app-manifests.md`](app-manifests.md), [`spa-hosting-tutorial.md`](spa-hosting-tutorial.md) | `AppManifest`, `AppPointerRecord`, `announce`/`find_providers`, plain content serving |
| Availability beyond the original publisher | [`p2p-app-caching-tutorial.md`](p2p-app-caching-tutorial.md) | Re-announcing fetched objects, cache eviction |
| Network bootstrapping | [`bootstrap-nodes-tutorial.md`](bootstrap-nodes-tutorial.md) | `dial`, `kad.add_address`/`bootstrap()`, first-contact discovery |
| Desktop chat | [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md) | Embedded `Runtime` in Tauri, gossipsub pub/sub, identity exchange, offline delivery via pointers |
| Desktop presence ("who's online") | [`tauri-presence-app-tutorial.md`](tauri-presence-app-tutorial.md) | `peers()`, lightweight gossipsub heartbeats, the smallest real `canopee-sdk` app |
| Desktop multiplayer game | [`tauri-multiplayer-game-tutorial.md`](tauri-multiplayer-game-tutorial.md) | Application-level ordering/sequencing on top of unordered delivery, state-divergence detection |
| Collaborative document editor | [`tauri-collab-editor-tutorial.md`](tauri-collab-editor-tutorial.md) | CRDT-based automatic merge (not detect-and-abort), snapshot persistence for late joiners |

Together, these already span: static content, live pub/sub messaging, two
different answers to "state might diverge" (abort vs. merge), and network
bootstrapping. The gap that remains is mostly about *storage patterns*
(versioning, multi-device) and *trust* (reputation, moderation) — see
below.

## 2. Deserves a tutorial — scoped, not yet written

### A blog/wiki with real version history

**Why it's different from what's already written:** `app-manifests.md`'s
`AppPointerRecord` only ever tracks the *latest* manifest — resolving a
pointer gives you the current version, with no way to list or browse
previous ones. A blog/wiki wants exactly that history. This would mean
extending the pointer pattern so each new manifest optionally references
its predecessor's `ObjectId` (a simple linked list of versions), and
building a "show history" UI that walks that chain backward from the
current pointer. Distinct enough from the existing app-manifest tutorials
to deserve its own: it's really a tutorial about *designing a versioning
scheme on top of immutable objects*, which nothing else here covers.

**Scoping note:** this is buildable without touching any crate's core
code — it's entirely CLI/app-layer design (a new manifest field,
essentially), which makes it a good "medium difficulty, no core changes
needed" tutorial to write next if the goal is breadth over depth.

### Multi-device sync for one identity

**Why it's different:** every existing tutorial assumes "one identity, one
device." A file-sync tool (your own notes/files, kept consistent across
your laptop and desktop) requires answering a question nothing in this
repo currently documents: **how do two of your own machines safely share
one identity's private key?** `Identity::create`/`load` in
[`crates/canopee-identity`](../crates/canopee-identity/README.md) treats
the key file as generated once, locally, and never mentions transferring
it. A real tutorial here would need to work through: exporting/importing
the raw key material securely (not just copying the file over an unencrypted
channel), what happens if both devices are online and edit the same file
at the same moment (the same convergence problem
[`tauri-collab-editor-tutorial.md`](tauri-collab-editor-tutorial.md) solves
with a CRDT — this is a legitimate reuse of that same technique for a
different app shape), and using `export`/`import` plus `find_providers`/
`fetch_object` for the actual sync mechanism.

**Scoping note:** the "same key on two devices" security question is worth
resolving explicitly and honestly (linking to
[`security-considerations.md`](security-considerations.md)'s point about
the key file having no encryption at rest today) before writing the
step-by-step — this one has a real design prerequisite the others don't.

### A P2P bulletin board / public forum

**Why it's different:** chat's pattern is 1:1 (or small-group) and
ephemeral-by-default with an opt-in persistence layer. A forum is the
opposite shape: many-to-many, public, and persistence is the *point* from
the start, not an afterthought. This would generalize
[`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md)'s Step 5
snapshot/pointer pattern from "one conversation's message history" to
"all posts under one public topic," and would need to address something
chat doesn't: unbounded growth of a public topic's history (unlike a
private conversation between known participants, anyone can post, so
there's no natural cap on how much content accumulates — this connects
directly to [`p2p-app-caching-tutorial.md`](p2p-app-caching-tutorial.md)'s
eviction-policy work, applied to a different kind of content).

**Scoping note:** worth writing after the caching tutorial exists in a
built form, since this app is a real-world consumer of exactly the
eviction policy that tutorial designs.

## 3. Interesting, not yet well-scoped

### A signed reputation/review system

**The idea:** identities leave signed reviews/ratings of other identities
or of published apps, aggregated into some kind of trust score.

**Why it's not scoped yet:** this one runs directly into
[`security-considerations.md`](security-considerations.md)'s stated gaps —
no revocation, no reputation mechanism for misbehaving peers, no defense
against a compromised or sock-puppet identity flooding fake reviews. A
tutorial here can't responsibly hand-wave those away the way, say, the
game tutorial can honestly scope out competitive-cheat-resistance and
still ship something useful for friends. A trust system whose own trust
model has an unsolved Sybil-attack problem (nothing stops one person from
generating many identities) isn't something to write a "here's how to
build it" guide for yet — it needs real design work (e.g. some kind of
proof-of-relationship or web-of-trust weighting) before a step-by-step
would be responsible to publish. Worth revisiting once
`security-considerations.md`'s key-management and reputation gaps have
actual proposed answers, not before.

### Real-time voice/video calling

**The idea:** peer-to-peer audio/video, the same "no server" pitch as
everything else here, using WebRTC-style media transport over the existing
libp2p connection.

**Why it's not scoped yet:** this needs a media transport Canopee doesn't
have at all today — `canopee-network`'s behaviours (gossipsub,
request-response, relay) are built for small discrete messages and
content-addressed object transfer, not continuous low-latency media
streams. Building this would mean either integrating a WebRTC stack
alongside libp2p (real, substantial systems work — codec negotiation,
jitter buffering, NAT traversal specifically tuned for media rather than
generic hole punching) or evaluating whether libp2p's own `webrtc`
transport (a real, separate libp2p feature not currently used anywhere in
`canopee-network`) fits. This is a multi-week engineering investigation
before it's a tutorial-sized topic — flagged here as a real, desirable
idea, deliberately not scoped further until someone's done that
investigation.

## Where this leaves things

If you're picking what to build or tutorialize next: the blog/wiki
versioning idea is the lowest-effort, highest-clarity next tutorial (no
core-crate changes, a clean extension of an existing pattern). Multi-device
sync and the bulletin board are good "next tier" candidates once you want
more storage-pattern breadth. Reputation systems and real-time media are
real, valuable ideas that need genuine design work — possibly their own
proposal/design docs, in the style of
[`roadmap-hosting-replacement.md`](roadmap-hosting-replacement.md) — before
they're ready to become a "follow these steps" tutorial at all.
