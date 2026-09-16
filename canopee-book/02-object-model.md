# Chapter 2: The Object Model

The object model is the heart of Canopee. Everything — profiles, contact lists, app manifests, file blobs, home indexes — is an object. Understanding objects is understanding Canopee.

## Object Structure

Every object in Canopee has the same structure:

```rust
struct Object {
    id: ObjectId,           // sha256(bincode(payload)) as hex string
    payload: ObjectPayload, // the actual data
    public_key: Vec<u8>,    // owner's protobuf-encoded Ed25519 public key
    signature: Vec<u8>,     // Ed25519 signature over bincode(payload)
}

struct ObjectPayload {
    owner: IdentityId,       // canopee://identity/<base58-peer-id>
    metadata: ObjectMetadata,
    object_type: ObjectType,
    data: Vec<u8>,           // the application-specific payload
}

struct ObjectMetadata {
    created_at: u64,            // unix timestamp
    size: u64,                  // bytes
    content_type: Option<String>, // MIME type or application-defined (None by default)
}
```

## Content Addressing

The object's ID is derived from its content:

```
ObjectId = hex(sha256(bincode(payload)))
```

This means:
- Two objects with identical payloads have the same ID
- Changing any byte in the payload produces a different ID
- The ID is a deterministic, tamper-evident fingerprint of the object

Content addressing has a profound consequence: **you can verify an object's integrity just by checking its ID**. If you request object `abc123...` and receive bytes that hash to `def456...`, you know the object is wrong — regardless of who sent it.

## Cryptographic Verification

Every object carries two proofs:

1. **Content integrity**: `id == sha256(bincode(payload))`
2. **Authorship**: `ed25519_verify(public_key, signature, bincode(payload))`

Both are checked on every operation:

- **On write** (`put_verified`): rejects objects that fail either check
- **On read** (`get_verified`): rejects objects that fail either check
- **On list** (`list_objects`): tolerates undecodable entries (so one corrupt file doesn't break the whole listing). Every decodable entry is verified eagerly, and the result is exposed as a `verified: bool` flag on each returned `ObjectInfo` — an entry that decodes but fails verification is still listed, flagged as unverified

This verification happens regardless of where the object came from — local disk, file import, or P2P fetch from a stranger. There is no "trusted source" shortcut. The math is the trust.

## Object Types

Objects carry a type tag that tells applications how to interpret the `data` field:

| Type | Purpose |
|------|---------|
| `Blob` | Arbitrary binary data — files, images, assets |
| `AppManifest` | Describes a published application (entrypoint + asset map) |
| `AppPointer` | Reserved — not currently constructed |
| `Profile` | User profile (display name, DH public key, avatar) |
| `ContactList` | List of contacts with names, peer IDs, and DH keys |
| `HomeIndex` | User's table of contents across all apps and devices |

The type tag is **not** a routing mechanism. The storage layer treats every object identically — it stores and retrieves bytes. The type tag is for application-layer interpretation. When you `get()` an object, you decode its `data` field based on the type:

```rust
let profile: Profile = profile_object.decode::<Profile>()?;
```

The type tags are append-only (never reordered) to keep the bincode serialization stable across versions.

## The Portable Unit: ExportBundle

The `ExportBundle` is the universal unit of exchange in Canopee:

```rust
struct ExportBundle {
    version: u32,     // currently 1
    object: Object,   // the complete, signed object
}
```

Everywhere objects leave their home — CLI export/import, P2P exchange, the `.canopee` file format — they travel as `ExportBundle`. This ensures that the signature, public key, and content hash travel with the data. The recipient can always verify the bundle independently.

## How Objects Are Stored on Disk

Objects are stored as flat files on disk, one file per object, named by their `ObjectId`:

```
~/.canopee/storage/
  a1b2c3d4e5f6...   (bincode-serialized Object)
  f7e8d9c0b1a2...   (another Object)
  ...
```

Writes use a tmp-then-rename pattern for atomicity. Since object IDs are content-addressed, two concurrent writes of the same object are safe — they write identical bytes.

The `list` operation reads the directory and returns all valid entries. It skips dot-files and tolerates undecodable entries. This is a deliberate resilience choice: one corrupt object file doesn't prevent other objects from being listed.

## Object Lifecycle

### Creating an Object

1. The application constructs the payload (e.g., a `Profile`, a `Blob`, a `HomeIndex`)
2. The payload is serialized with bincode
3. The SHA-256 hash of the serialized payload becomes the `ObjectId`
4. The node signs the serialized payload with the owner's Ed25519 private key
5. The complete `Object` (id + payload + public_key + signature) is written to disk

### Sharing an Object

Sharing is opt-in. An object becomes visible to peers when:

1. It is **announced** to the DHT (providing a DHT provider record that maps the object ID to the announcing peer)
2. It is listed in the `HomeIndex` with `shared: true`

The `share_object` runtime method flips the `shared` flag on a `HomeIndex` entry and announces the object to the DHT. The `set_home_entry_shared` method does the same from the CLI/SDK.

### Fetching an Object

When you fetch an object from a peer:

1. You request the object by its `ObjectId`
2. The peer responds with an `ExportBundle`
3. You verify the content hash and signature
4. You choose whether to import it into your local store

Import is a separate step from fetch. You might fetch an object to inspect it without adding it to your store. This composability is intentional.

### Re-serving Cached Objects

Objects cached from peers are always re-served to other peers who request them. This is the distributed cache — by fetching an object, you become a provider for it. The cache has a 256 MiB cap with LRU eviction (30-second sweep interval).

## What Makes This Different

The object model has several properties that distinguish it from traditional file storage:

- **Tamper-evident**: you can't modify an object without invalidating its ID
- **Author-verifiable**: you can always prove who created an object
- **Self-contained**: an object carries everything needed to verify it
- **Portable**: objects move between nodes, applications, and devices without loss of provenance
- **Resilient**: content addressing means identical objects deduplicate naturally across peers

This is the foundation. Everything else — records, apps, profiles, chat — is built by composing objects.
