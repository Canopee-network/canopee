# Tutorial: distributed app hosting via a fetch cache

This is a hands-on, step-by-step guide to a specific feature: making a
Canopee app (published via [`app-manifest`](app-manifests.md)) stay
reachable even after the original publisher's node goes offline, by having
every node that *fetches* the app also start *serving* it to others.

No code is given here — each step describes what to build, which existing
functions to hook into, and how to verify it worked before moving to the
next step. Read [`app-manifests.md`](app-manifests.md) first if you haven't
— this tutorial assumes you're comfortable with `announce`/`find_providers`,
`fetch_app`, and `AppPointerRecord`.

## The problem, concretely

Right now:

1. Alice publishes a portfolio. `app-manifest` announces her manifest and
   every asset — the DHT now has provider records saying *Alice's peer id*
   has these objects.
2. Pierre runs `open --owner alice --name alice-portfolio`. His node fetches
   every object from Alice, imports them into his own `~/.canopee/storage`,
   and serves them locally.
3. Alice's node crashes.
4. A third person, Sam, tries to open the same app. `find_providers` still
   only returns Alice's peer id (that's the only provider record that was
   ever published) — Alice is offline, so the fetch fails, even though
   **Pierre's node has a complete, verified copy sitting on disk right
   now.**

The fix has two halves:
- **Announce on fetch** — Pierre's node should tell the DHT "I have this
  object too" right after importing it, not just Alice.
- **Don't keep everything forever** — once nodes start hosting every app
  they've ever viewed, disk usage grows unbounded. You need a policy for
  what to evict and when.

We'll build both, in order, with a manual test after each step.

## Step 0: orient yourself in the existing code

Before changing anything, find and read these three functions — you'll be
extending all of them:

- `get_or_fetch` in [`crates/canopee-cli/src/app.rs`](../crates/canopee-cli/src/app.rs)
  — the single place every fetched object passes through `client.import(...)`.
- `CanopeeClient::announce` in [`crates/canopee-sdk/src/client.rs`](../crates/canopee-sdk/src/client.rs)
  — already does exactly what you need to call; you're not writing new
  DHT logic, just calling this in a new place.
- `Storage::list_objects` in [`crates/canopee-storage/src/storage.rs`](../crates/canopee-storage/src/storage.rs)
  — how the codebase currently enumerates what's on disk. You'll be
  deciding whether this is enough metadata for eviction, or whether you
  need more (see Step 3).

**Checkpoint:** you should be able to explain, in one sentence each, what
`get_or_fetch`, `announce`, and `list_objects` currently do, before
continuing.

## Step 1: announce whatever you fetch

**Goal:** after Pierre's node imports an object it fetched from Alice, it
also announces itself as a provider of that object — so `find_providers`
starts returning *both* Alice and Pierre.

**Where:** `get_or_fetch` in `crates/canopee-cli/src/app.rs`. Look at the
line that calls `client.import(bundle.clone()).await?;` — that's the moment
an object becomes locally available. Right after it succeeds, that's where
a new `client.announce(id.clone()).await?;` call belongs.

**Design questions to answer yourself before coding:**
- Should announcing be best-effort (log a warning and continue if it fails)
  or should a failed announce fail the whole fetch? Think about what you'd
  want to happen if you're offline from the DHT but the object still
  imported fine locally — is the fetch itself unsuccessful in that case?
- `fetch_app` currently fetches assets concurrently via `try_join_all`. If
  you add an `announce` call inside `get_or_fetch`, does that concurrency
  still make sense, or does announcing turn into a bottleneck? (Hint: think
  about whether `announce` blocks on network round trips the same way
  `fetch_object` does, and whether that's still fine to run N-at-once.)

**How to verify it worked**, using the same two-node setup from
[`app-manifests.md`](app-manifests.md#end-to-end-alice-publishes-pierre-opens-it-alice-republishes):

1. Start Alice's and Pierre's nodes, dial them together, publish from
   Alice, `open` from Pierre.
2. From Pierre's node, run `canopee list` — the fetched objects should now
   be sitting in his `~/.canopee/storage` (already true before this step;
   just confirms Pierre actually has them).
3. Run `find_providers` for the manifest's object id — either add a quick
   `find-providers <id>` CLI invocation if one doesn't exist yet, or use
   the [`canopee-sdk` example app](../crates/canopee-sdk/README.md)
   (`cargo run -p canopee-sdk --example app -- find-providers <object-id>`).
   Before this step, only Alice's peer id comes back. After it, **both**
   Alice's and Pierre's peer ids should be listed as providers.
4. This is your real test: kill Alice's node entirely (`canopee stop` or
   just kill the process). Bring up a third node (Sam), dial it to *Pierre*
   only (not Alice — she's dead), and run
   `open --owner alice-identity --name alice-portfolio --peer <pierre-peer-id>`.
   It should succeed and serve Alice's content, sourced entirely from
   Pierre.

If step 4 works, you've built the core of distributed hosting. Everything
after this is about making it survive at scale rather than just work once.

## Step 2: don't just take the first provider — try them all

**Goal:** right now, `resolve_peer` in `app.rs` takes
`providers.into_iter().next()` — the *first* provider `find_providers`
happens to return. Once Step 1 means an object can have many providers,
"the first one" might be offline, slow, or unreachable (e.g. behind a NAT
you can't punch through). Fetching should try multiple candidates before
giving up.

**Where:** `resolve_peer` and `get_or_fetch` in `app.rs`.

**What to change:** instead of resolving to a single `peer_id: String` and
committing to it, have `get_or_fetch` iterate over the full provider list
returned by `find_providers`, attempting `fetch_object` against each in
turn, and only erroring out once *all* candidates have failed. Think about:
- What order should you try them in? (Simplest: DHT-returned order plus
  maybe put a caller-supplied `--peer` first if given, since that's an
  explicit hint.)
- Should a failed attempt against one peer count against that peer for
  future fetches in the same `fetch_app` call, or is each object
  independent? (Given Step 1, different objects might genuinely have
  different provider sets, so probably don't assume "the peer that worked
  for the manifest will work for every asset.")

**How to verify:** with three nodes (Alice, Pierre, Sam) all having
announced the same object (Alice originally, Pierre after Step 1), kill
Alice specifically and confirm a *fresh* node with no prior `--peer` hint —
just relying on `find_providers` — still succeeds by falling through to
Pierre.

## Step 3: distinguish "mine" from "cached"

Before you can safely delete anything, you need to know what's safe to
delete. Right now `Storage`/`ObjectInfo` treats every object on disk
identically — there's no field recording *why* it's there.

**Goal:** be able to answer, for any object in `~/.canopee/storage`: did
this node's own identity create it (via `put`/`app-manifest`), or did it
arrive via `get_or_fetch` because someone else asked for it?

**Where to think about this:** `ObjectInfo` and `ObjectPayload` in
`crates/canopee-storage/src/object.rs`, and `Storage::put_verified`/`import`
in `storage.rs`.

**Two designs, pick one and justify it to yourself:**
- **Compare `payload.owner` to your own identity.** An object you fetched
  from someone else will always have a different `owner` than your own
  `IdentityId` — you never sign objects you didn't create. This needs zero
  new fields; `Runtime`/`Storage` already know your own identity. But: it
  conflates "not mine" with "cached," which happens to be true today but
  is a coincidence, not a guarantee — nothing stops a future feature from
  having you `import` an object you *do* want to keep permanently even
  though someone else owns it.
- **Add explicit metadata on import** (e.g. a `cached_at: u64` timestamp,
  separate from `created_at`, only set by the `import` path, not `put`).
  More honest about intent, but requires a schema change: either a new
  field on `ObjectPayload` (careful — that's hashed into `ObjectId`, so
  adding a field there changes every future object's id computation and
  breaks compatibility with already-published objects) or a **separate
  sidecar** — e.g. a small file or embedded database recording
  `object_id -> last_accessed` outside the signed `Object` itself.

Given the hashing concern, the sidecar approach is very likely the right
call — work through *why* modifying `ObjectPayload` is risky before you
commit to a design (re-read `ObjectId::from_payload` in
`crates/canopee-storage/src/object_id.rs` if you're unsure why).

**Checkpoint:** sketch (on paper, in a comment, however you like) the
sidecar's shape — what needs to be recorded per cached object, and where
it lives on disk relative to `storage.root()`. Don't write the eviction
logic yet — that's Step 4.

## Step 4: evict cached objects, never your own

**Goal:** a background sweep that deletes cached (not owned) objects when
some limit is exceeded — total cache size, object count, or age, your
choice — while never touching anything the local identity created.

**Where to hook it in:** `canopee-node`'s `Node::run` in
`crates/canopee-node/src/lib.rs`. Look at how `Node::run`'s main loop
already uses `tokio::select!` around `listener.accept()` and
`shutdown.recv()` — a periodic eviction sweep is a third arm of that same
`select!`, driven by a `tokio::time::interval`.

**Questions to work through:**
- What's your eviction policy? Simplest to reason about: total bytes over
  a configurable cap, evict least-recently-served first (which is exactly
  why Step 3's sidecar needs a `last_accessed`-style timestamp, updated
  every time an object is served — either on `get_verified` reads or
  specifically when `canopee-node` serves a `FetchObject` request to
  another peer, since that's the "someone's actually using my cached copy"
  signal).
- What happens to the DHT provider record for an object you evict? If you
  announced it in Step 1 and then delete it locally, `find_providers` will
  still list you as a provider until that record expires (libp2p-kad
  auto-expires/re-publishes provider records on its own timers — see
  `provider_record_ttl`/`provider_publication_interval` in the `kad::Config`
  used by `NetworkManager::new` in `crates/canopee-network/src/manager.rs`
  — you don't have to build re-announcement yourself, but you should know
  it's happening). A peer that then asks you for that object will get a
  `NotFound` from your `ObjectProvider::get_object` implementation
  (`StorageObjectProvider` in `crates/canopee-runtime/src/lib.rs`) — is
  that an acceptable failure mode, or do you need to actively withdraw the
  provider record on eviction? (There's a `stop_providing` on `kad::Behaviour`
  if you decide you need it — you'd wire it through `NetworkManager` the
  same way `announce`/`put_record` already are.)
- Should eviction run on a fixed interval, or only when a cap is actually
  exceeded (e.g. checked right after every `import`)? Either is
  defensible; a periodic sweep is simpler to reason about and matches the
  pattern `Node::run` already uses elsewhere.

**How to verify:** publish several apps with large-ish binary assets from
one node, fetch them all from a second node with a deliberately tiny cache
cap, and confirm: (a) the oldest/least-used cached objects disappear from
`canopee list` on the fetcher once the cap is exceeded, (b) anything the
fetcher itself originally published via its own `app-manifest` is *never*
evicted regardless of the cap, and (c) a subsequent `find_providers` for an
evicted object either stops listing the evicting node (if you implemented
`stop_providing`) or the evicting node correctly answers `NotFound` rather
than crashing when asked for something it no longer has.

## Step 5 (stretch): make pointer resolution provider-aware

Once objects can live on multiple nodes, revisit
`resolve_app_pointer`/`AppPointerRecord` from
[`app-manifests.md`](app-manifests.md#resolving-by-name-app-pointers). The
pointer only ever names the *manifest id* — it says nothing about who's
serving it. Right now `open --owner --name` still needs a `--peer` hint (or
a lucky `find_providers` hit) to actually fetch anything.

Think about whether it's worth teaching `open` to:
- Resolve the pointer (gets you the manifest id),
- Then immediately run `find_providers` on *that* id as a fallback when no
  `--peer` was given and the pointer's original owner is unreachable —
  this should already work for free once Step 1 is in place, since
  Pierre/Sam will show up as additional providers. Confirm this rather
  than assume it — write a test where the pointer's owner is offline but
  the manifest is still resolvable purely through cached providers.

This step is mostly verification, not new code — a good exercise in
confirming the earlier steps actually compose the way you designed them to.

## Summary checklist

Work through these in order; each depends on the previous one working:

- [ ] Step 1 — `get_or_fetch` announces after every successful import
- [ ] Step 2 — fetching tries multiple providers, not just the first
- [ ] Step 3 — cached objects are distinguishable from owned ones
- [ ] Step 4 — a bounded cache with an eviction policy that never touches
      owned objects
- [ ] Step 5 — confirm pointer resolution survives the original publisher
      going offline, once enough peers have cached the app

By the end, killing the original publisher's node mid-demo should no
longer take the app down with it — that's the concrete, demoable proof
this feature works.
