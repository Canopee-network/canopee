# Objects & pointers

Canopee stores data as signed, content-addressed **objects**. Objects are
immutable (like git blobs), which makes them easy to verify, cache, and
deduplicate. Because they're immutable, every *change* goes through **pointers**
— signed records that point a stable `(owner, name)` at the latest version of
an object.

This is the whole data model. Everything else is built on top of it.

## Objects

An `Object` (`canopee-storage/src/object.rs`) is:

- a blob of bytes (`data`),
- an `ObjectType` tag (Blob, File, Profile, HomeIndex, CapabilityIndex, …),
- a signature from the creator's identity key,
- an `ObjectId` = the hash of (type + data), so any object's id is
  self-certifying.

Property: **`id = hash(type || data)`**. You can verify an object you fetched
over the network without trusting the sender — recompute the hash, verify the
signature against the advertised owner, and both must match.

Objects are stored on disk under `~/.canopee/storage/<id>` and evicted from
the on-node LRU cache based on `cached_bytes`.

## Pointers

A `PointerRecord` (`canopee-storage/src/pointer.rs`) is a signed record of the
form

```
(owner, name) → object_id  (+ timestamp, signature)
```

The interesting property is that **the pointer is "owned" by whoever its
`owner` field is**, but the object it points at can be signed by a different
identity. This is the crux of the sharing model:

- *records* are published under a reserved `(owner, name)` key (e.g.
  `(owner, "profile")`, `(owner, "home")`, `(owner, "devices")`), treated as
  *reserved* names;
- *apps* publish their own pointers under `app:<name>`;
- *shares* create `(owner, "entry:<name>")` pointers to a *peer's* object.

So a pointer answers "what is the latest object for this name, under this
owner?" using only the owner's signature.

## Records

A **record** is a convention: a reserved `(owner, name)` pointer whose value
is the latest *immutable* object for that piece of person-level data. Because
the pointer is signed by the owner and the object is content-addressed, a
record is always both fresh and verifiable.

The reserved record names (in `canopee-storage/src/user.rs`):

| Record | Pointer key | Value | Meaning |
|---|---|---|---|
| Profile | `(owner, "profile")` | `Profile` | display name, DH key, avatar, version |
| Contacts | `(owner, "contacts")` | `ContactList` | people you know |
| Home index | `(owner, "home")` | `HomeIndex` | what you've stored/shared (see below) |
| Username | `(owner, "username")` | `UsernameRecord` | your globally-unique claim |
| Devices | `(owner, "devices")` | `DeviceList` | which machines carry your identity |
| Capabilities | `(owner, "capabilities")` | `CapabilityIndex` | grants you've issued (see [Capabilities](capabilities.md)) |

Global Kademlia DHT records (not `(owner, name)` pointers) round it out:

| Registry | Key | Value |
|---|---|---|
| Username registry | `username:<name>` | verified owner identity |
| Device registry | `device:<peer-id>` | verified owner identity |

## The home index

`(owner, "home")` → a signed `HomeIndex` — the table of everything you've
stored under a name:

```
name  →  object_id  +  shared: bool
```

- `~/.canopee/home` is just an alias into it (the CLI `home` command lists it).
- When you `put --name` an object, you upsert a `shared: false` entry.
- When you `share` an object, you flip it to `shared: true` and publish a
  `(owner, "entry:<name>")` pointer **plus** announce the object on the DHT.
- When you `unshare`, you withdraw the DHT announcement and stop serving it;
  the entry is removed from the public pointer set.

Sharing is described in [Sharing](sharing.md); the walk-through is in
[the sharing guide](../guides/sharing.md).

## App pointers

An app that wants to be installed publishes an `AppPointerRecord` under
`(owner, "app:<name>")` pointing at an `ApplicationManifest` object. Anyone
can `canopee open --owner <id> --name <name>` (or `resolve_app_pointer` /
`publish_app_pointer` in the SDK) to resolve and install it. See
[Publishing apps guide](../guides/publishing-apps.md).

## Plumbing: how a pointer is written and read

Writing a record (runtime `publish_pointer` in
`crates/canopee-runtime/src/records.rs`):

1. build the object (signed),
2. `Storage::put_verified` it,
3. sign a `PointerRecord{owner, name, object_id, timestamp}`,
4. `Storage::put_pointer` (persist locally),
5. *announce* the pointer on the DHT so peers can resolve it,
6. if marked shared, serve the object to peers on request.

Reading a record (`resolve_pointer`):

1. look up the pointer by `(owner, name)` — locally first, then the DHT,
2. fetch the pointed-to object (cache, storage, or from a provider peer —
   `fetch_object`, bounded by `RESOLVE_DHT_TIMEOUT`),
3. verify hash + signature before returning.

Write-then-publish means a record update is atomic from the network's point
of view: peers that can't reach you still have the previous signed version,
and resolve to your *latest reachable* object.