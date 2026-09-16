# Chapter 5: Mutable Records

## The Immutability Problem

Objects in Canopee are immutable. Once created, they cannot be changed. This is great for integrity — but it creates a problem: how do you represent something that changes over time?

Your profile changes when you update your display name. Your contact list changes when you add a friend. Your app's latest version changes when you publish an update. If the object is immutable, how do you point to "the latest version"?

## Records: Mutable Pointers to Immutable Objects

The answer is **records**. A record is a signed, mutable pointer that maps a `(owner, name)` pair to the latest version of an object:

```
(owner IdentityId, name: String) → Object
```

Records are the only mutable thing in Canopee. Editing means writing a new object version and repointing the record. The record envelope itself is a lightweight signed structure (an `AppPointerRecord`) — the version number lives inside the referenced object, not the envelope, and the DHT entry is overwritten on each publish.

## How Records Work

### Structure

Each record type follows the same pattern:

```rust
struct RecordType {
    // ... record-specific fields ...
    version: u64,  // incremented on each save
}
```

The record is wrapped in a signed envelope and stored both locally (in `~/.canopee/records/`) and on the DHT.

### Publishing

When you publish a record:

1. The node constructs the record object (with an incremented version number)
2. The node signs it with the owner's private key
3. The record is saved to the local `records/` directory (authoritative)
4. The record is published to the DHT in the background (fire-and-forget)

The local cache is authoritative. The DHT put runs asynchronously — Kademlia queries can take up to 60 seconds and must never stall a local save.

### Resolution

When you resolve a record:

1. Check the local `records/` cache first
2. If not found locally, query the DHT (bounded at 10 seconds)
3. Verify the record's signature before trusting it

The local cache ensures instant resolution for records you've published. DHT resolution is for records published by others.

### DHT Key Derivation

The DHT key for a record is derived deterministically:

```
key = sha256("canopee-app-pointer:" + owner + ":" + name)
```

This means anyone who knows the `(owner, name)` pair can derive the DHT key independently. Records can arrive from arbitrary DHT peers, so the signature must be verified before trusting the embedded object reference.

## Built-in Record Types

### Profile

```rust
struct Profile {
    display_name: String,
    dh_public_key: [u8; 32],  // X25519 for E2E encryption
    avatar: Option<ObjectId>, // optional avatar image
    version: u64,
}
```

The profile is pointed to by the record `(owner, "profile")`. It contains the user's display name, their DH public key for encrypted communication, and an optional avatar.

### ContactList

```rust
struct ContactList {
    contacts: Vec<Contact>,
    version: u64,
}

struct Contact {
    name: String,
    peer_id: String,
    dh_public_key: [u8; 32],
    note: Option<String>,
}
```

Pointed to by `(owner, "contacts")`. This is your address book — the peers you've chosen to communicate with, along with their DH keys for encrypted messaging.

### HomeIndex

```rust
struct HomeIndex {
    profile: Option<ObjectId>,
    contacts: Option<ObjectId>,
    entries: Vec<HomeEntry>,
    version: u64,
}

struct HomeEntry {
    name: String,
    object: ObjectId,
    object_type: ObjectType,
    shared: bool,
    app: Option<String>,
}
```

Pointed to by `(owner, "home")`. The HomeIndex is the user's table of contents — a manifest of everything they have stored across all applications and devices. Each entry has:

- `name`: human-readable label
- `object`: the ObjectId of the referenced object
- `object_type`: what kind of object it is
- `shared`: whether this object is served to peers
- `app`: which application created this entry (optional)

The `shared` flag is the sharing gate. When `shared` is `true`, the object is announced to the DHT and served to requesting peers. When `false`, it stays private.

### AppPointer

The `AppPointer` object type tag is reserved and never constructed as an object. However, the *record envelope* used for every record — the `AppPointerRecord` — is the universal mechanism described throughout this chapter: it maps `(owner, name)` to an `ObjectId` and is constructed on every publish. App manifests are resolved through it under the record `(owner, app-name)`.

## The Sharing Gate

The sharing system is built on records and the HomeIndex:

1. **Share an object**: `share_object` adds a `HomeEntry` with `shared: true` to the `HomeIndex`, announces the object to the DHT
2. **Unshare an object**: `set_home_entry_shared(entry, false)` flips the flag, unannounces the object
3. **Serving**: the storage layer serves only objects that are in the shared set (via `share_object` / `announce`). Note: objects cached from peers are *always* re-served — that re-hosting is the distributed cache — but your own objects are gated on the shared set
4. **Restart**: on startup, the runtime loads persisted HomeIndex entries and re-announces all `shared: true` objects

This means sharing survives restarts. The local HomeIndex is the source of truth; the DHT is best-effort.

## Version Management

Version numbers are incremented automatically by the node. Clients send raw data; the node:

1. Loads the current record (if any)
2. Increments the version number
3. Signs the new record
4. Saves it locally
5. Publishes to the DHT

This ensures version numbers are incremented by the node and signed by the correct key — no client can forge a version bump. Note, however, that version is currently treated as informational: nothing on the resolution path compares a record's `published_at` against a previously-seen value, so an old (validly signed) record replayed in place of a newer one is not yet rejected. See Chapter 21.

## Record Signing

Records are signed by the owner's private key. The signing bytes are constructed from the record's fields:

```
"{name}:{owner}:{manifest}:{published_at}"
```

The signature includes all fields that could be tampered with. Note there is no version in the signed bytes — the envelope has no version field; versions live inside the referenced objects. The `verify()` method checks:

1. The signature is valid under the embedded public key
2. The embedded public key derives the claimed owner's `PeerId`

This second check prevents an attacker from signing a record with their own key and claiming it belongs to someone else.
