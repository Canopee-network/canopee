# canopee-storage

Content-addressed, signed object storage for a Canopee node. Every object is
owned by an [identity](../canopee-identity), self-verifying (its ID is the
hash of its contents, and its signature proves who created it), and portable
(it can be exported to a single file and imported by any other node without
re-establishing trust out of band).

## Object model

```
Object
├── id: ObjectId              sha256(bincode(payload)), hex-encoded
├── payload: ObjectPayload
│   ├── owner: IdentityId     who created this object
│   ├── metadata: ObjectMetadata   created_at, size, content_type
│   ├── object_type: ObjectType    Blob | AppManifest | AppPointer
│   └── data: Vec<u8>         the actual bytes (opaque for Blob, bincode(AppManifest) for AppManifest)
├── public_key: Vec<u8>       owner's protobuf-encoded public key
└── signature: Vec<u8>        Ed25519 signature over bincode(payload)
```

`object_type` is a tag, not a routing mechanism — `Storage` treats every
`Object` identically regardless of type. It only matters to callers:
`Object::decode::<T>()` deserializes `data` as whatever type the caller
expects (e.g. `AppManifest`), and `object_type()` lets a caller check the
tag before assuming what's in `data`.

An object is valid iff:
1. `id == sha256(bincode(payload))` (the ID matches its content — `verify_id`)
2. `signature` verifies against `payload` under `public_key` (`Verify::verify`)

Both checks run on every read and write through `Storage`, so a corrupted or
tampered object on disk is caught before it's ever handed back to a caller.

## API

```rust
use canopee_storage::{Object, ObjectType, Storage, ObjectId};

let storage = Storage::new("~/.canopee/storage");

// Create + store, signed by `identity` (an `&canopee_identity::Identity`).
let object = Object::new(&identity, b"hello canopee".to_vec(), ObjectType::Blob);
// or, for plain blobs: Object::blob(&identity, b"hello canopee".to_vec());
storage.put_verified(&object).await?;

// Read back — fails if the signature or content hash don't check out.
let object = storage.get_verified(&object.id).await?;

// List everything, with per-object verification status.
let infos: Vec<ObjectInfo> = storage.list_objects().await?;

// Cheap existence check (no deserialization / verification).
if storage.exists(&object.id).await { /* ... */ }
```

### Exporting and importing

An `Object` can be wrapped in an `ExportBundle` (currently just `{ version,
object }`) for sharing outside the local store — e.g. writing to a file, or
sending over the network:

```rust
use canopee_storage::Export;

let bundle = object.export()?;               // fails if the object doesn't verify
let bytes = bincode::serialize(&bundle)?;      // write this to a `.canopee` file
// ... elsewhere, or on another node ...
let bundle: ExportBundle = bincode::deserialize(&bytes)?;
storage.import(&bundle.object).await?;         // re-verifies before writing
```

This is exactly how [`canopee-node`](../canopee-node)'s `Export`/`Import`
commands and [`canopee-network`](../canopee-network)'s peer-to-peer object
fetch both work — the bundle is the unit of exchange in every direction
(file, CLI, or network).

### App manifests and pointers

`AppManifest` is the type this crate provides for publishing a small static
app (e.g. a portfolio site) as one discoverable unit: a name, an owner, an
`entrypoint` object id (the app's `index.html`), and an `assets` map of URL
path → object id for everything else. It doesn't hold file bytes itself —
each file is its own `Object` (`ObjectType::Blob`), and the manifest is
stored as its own `Object` with `ObjectType::AppManifest`:

```rust
use canopee_storage::{AppManifest, Object};

let manifest = AppManifest { name, owner, entrypoint, assets };
let manifest_object = Object::app_manifest(&identity, &manifest)?;
storage.put_verified(&manifest_object).await?;
```

Because `ObjectId`s are content-addressed, republishing an app under new
content produces a brand-new manifest id — there's no way to overwrite a
manifest in place. `AppPointerRecord` solves that: a signed, mutable pointer
from a stable `(owner, name)` pair to the *latest* manifest id, meant to be
published as a DHT record (not through `Storage` — see
[`canopee-network`](../canopee-network)'s `put_record`/`get_record`) rather
than stored as an `Object`:

```rust
use canopee_storage::AppPointerRecord;

let pointer = AppPointerRecord::sign(&identity, "alice-portfolio", manifest_id)?;
let key = AppPointerRecord::key(&identity.id().clone(), "alice-portfolio");
// key/pointer then go over the network, not through Storage — see
// canopee-node's PublishAppPointer/ResolveAppPointer handlers.

assert!(pointer.verify()); // checks the signature *and* that public_key really is `owner`
```

Fetchers who only know `(owner, name)` derive the same `key` independently
(no need to exchange it out of band) and must call `verify()` on any
resolved record before trusting `record.manifest` — a record can arrive
from an arbitrary peer over the DHT, not just its actual owner.

| Type | Purpose |
|---|---|
| `Object` | The full signed, content-addressed object |
| `ObjectId` | `sha256` hex digest of the object's payload |
| `ObjectType` | Tag on an object's payload: `Blob`, `AppManifest`, or `AppPointer` (reserved — pointers are DHT records, not `Object`s; see below) |
| `ObjectPayload` | Owner + metadata + object type + raw bytes (what actually gets hashed/signed) |
| `ObjectInfo` | Lightweight listing view: id, owner, size, verified |
| `ExportBundle` | `{ version, object }` — the portable unit for import/export |
| `AppManifest` | Name + owner + entrypoint object id + `path -> object id` asset map for a published app |
| `AppPointerRecord` | Signed, mutable `(owner, name) -> manifest id` pointer, published as a DHT record rather than an `Object` |
| `Storage` | Filesystem-backed store: `put_verified`, `get_verified`, `list`, `list_objects`, `exists`, `import` |
| `Verify` / `Export` | Traits implemented by `Object` for verification and bundling |

## On-disk layout

`Storage::new(root)` treats `root` as a flat directory; each object is one
file named after its `ObjectId`, containing the `bincode`-serialized
`Object`:

```
storage/
├── 1b1832a6ad374a5d9626eb7dbcdc37f530335d2c3a43cc4f1bcccf76de73312f
├── 3041bc1432545ec0a867f63a4e538886e19faede25e4f1e138434d5ab4aa4882
└── ...
```

## Design notes

- `ObjectId` is content-derived, not random — two nodes that independently
  create identical payloads (same owner, same metadata, same bytes) get the
  same ID. In practice `created_at` timestamps make collisions between
  distinct puts vanishingly unlikely, but don't rely on ID uniqueness as a
  distinctness guarantee for otherwise-identical payloads.
- Signature verification happens on every `get_verified`/`list_objects`, not
  just on write. This is deliberate: objects arrive from three places (local
  `put`, file import, and peer-to-peer fetch over
  [`canopee-network`](../canopee-network)), and only two of those are
  locally trusted. Verifying uniformly means callers never need to know
  which path an object came from.
- `Storage` has no cache, index, or garbage collection — it's a direct
  filesystem mirror of "one object, one file." Listing (`list`/
  `list_objects`) is O(n) over the directory.
- `exists` uses `fs::try_exists(path).await.unwrap_or(false)`, not
  `.is_ok()` — `try_exists` returns `Ok(false)` (not an `Err`) when the path
  is simply absent, so `.is_ok()` was `true` in both the "exists" and
  "doesn't exist" cases. That bug made `Runtime::import` (which checks
  `exists` before importing) refuse every import with "Object already
  exists," silently breaking peer-to-peer fetch entirely.
- `AppPointerRecord` deliberately isn't an `Object` / doesn't go through
  `Storage` at all — it's not content-addressed (its whole point is being
  mutable at a fixed key) and it isn't "owned" by local storage the way a
  fetched `Object` is. `ObjectType::AppPointer` exists as a reserved tag but
  nothing currently constructs an `Object` with it.

## Testing

```bash
cargo test -p canopee-storage
```
