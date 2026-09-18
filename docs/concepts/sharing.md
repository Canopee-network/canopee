# Sharing

**Nothing is shared by default.** An object you `put` is stored locally and
signed by you, but it is invisible to the network until you explicitly share
it. There is no crawlable index, no "public by default" bucket, and no
central service to query.

This page covers the concrete sharing primitives. For the command-by-command
walk-through see [the Sharing guide](../guides/sharing.md).

## The two halves of "sharing"

1. **Publishing a pointer** — makes the *name* resolvable: anyone who knows
   your identity can resolve `(owner, "entry:<name>") → object` and fetch the
   object.
2. **Announcing a provider** — makes the *object id* discoverable: anyone who
   knows your object id can `find-providers <id>` and learn who serves it.

A share (`share` in the runtime, `crates/canopee-runtime/src/sharing.rs`)
does both atomically:

```
share_object(name, object_or_source):
  1. upsert (owner, name) → object_id in the home index with shared:true
  2. publish the (owner, "entry:<name>") pointer           (name is resolvable)
  3. announce <object_id> on the DHT                       (id is discoverable)
  4. mark the entry shared in the PersistableIndex
```

From the SDK: `share_object(id, name)` + `set_home_entry_shared(name, true)`.
From the CLI: `canopee share <name> [object]`.

## Fetching

Consumers fetch by either form:

- **by name** — `canopee fetch <peer/identity/username> <name>` resolves the
  `(owner, "entry:<name>")` pointer via the DHT, verifies the owner's
  signature, then pulls the object from a provider.
- **by id** — `canopee fetch <peer> <hex-id>` pulls `<hex-id>` from that peer
  using the direct object-exchange protocol.

In both cases the pulled object is content-verified (hash + signature) before
being stored (`Storage::put_verified`).

## Unsharing

`unshare(name)` / SDK `unshare(name)`:

1. withdraws the DHT provider announcement for the object id,
2. removes the `(owner, "entry:<name>")` pointer,
3. marks the home entry `shared: false`.

The object itself stays in your storage — unsharing only stops *serving*
it. Search for it stops working; existing copies on other peers (imo staleness
is unavoidable without GC) remain but are no longer refreshed.

## The home index as the share ledger

`(owner, "home")` (a signed `HomeIndex`) is the local source of truth of what
you've stored and shared:

```
name → { object_id, shared: bool }
```

It's a signed record like any other, so it syncs across your devices and can
be re-derived by any of your apps. The CLI's `canopee home` lists it; the
runtime re-shares all `shared:true` entries on node start — this **reseed**
(`reshare_public_user_records` in architecture terms) is what keeps an object
reachable even after a restart, and is the mechanism called at `Runtime::open`.

## Records are shared too

Person-level records (profile, contacts, home, devices, username,
capabilities) are also objects with pointers — and they're *always* shared so
that peers can resolve them. On open, the runtime re-publishes the pointer set
and re-announces the provider set for those records, so a peer can always
resolve a person even if their device was off for a while.

## Fetching makes you a provider

When you fetch an object you don't otherwise hold, the runtime stores it
locally and — like the content-cache note in
[Networking](networking.md) — it may be served to future requesters (within
`cached_bytes`, evicted by the LRU). In practice this is how content
replicates across the network without a server: fetching *is* seeding.

## Capabilities gate sharing of private resources

Sharing gives away *availability*; not all who can fetch should necessarily
*read*. When you want to grant a specific peer access to a resource (your
home entry name, a channel, a specific object), issue a signed capability —
see [Capabilities](capabilities.md). The runtime's `check_access` enforces
it before serving the resource.