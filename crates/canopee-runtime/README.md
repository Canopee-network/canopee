# canopee-runtime

Ties [identity](../canopee-identity), [storage](../canopee-storage), and
[network](../canopee-network) together into a single `Runtime` — everything
a Canopee node needs to actually run, minus the transport that exposes it to
clients (that's [`canopee-node`](../canopee-node)'s job).

## What `Runtime::open()` does

```rust
use canopee_runtime::Runtime;

let runtime = Runtime::open().await?;
```

On first run, this:
1. Creates `~/.canopee/` and its subdirectories (see
   [`canopee-config`](../canopee-config))
2. Generates and persists a new identity, or loads the existing one
3. Loads or initializes `NodeState` (identity, timestamps, started flag)
4. Opens the object [`Storage`](../canopee-storage)
5. Starts the [`NetworkManager`](../canopee-network), wired to a
   `StorageObjectProvider` so peers can fetch objects this node holds

Everything after that is a method call on `Runtime`: `put`, `get`, `list`,
`export`/`import`, plus direct access to `runtime.network` for the
peer-to-peer operations (`dial`, `announce`, `find_providers`, `get_object`,
`publish`, `subscribe`, ...).

```rust
let id = runtime.put(b"hello canopee".to_vec()).await?;
let object = runtime.get(&id).await?;
let objects = runtime.list().await?;

let bundle = runtime.export(&id).await?;         // in memory
runtime.export_to_file(&id).await?;               // also writes exports/<id>.canopee
runtime.import(bundle).await?;                    // rejects if the id already exists locally

runtime.network.announce(id).await?;
```

## `StorageObjectProvider`

`canopee-network`'s `ObjectProvider` trait is what lets the swarm answer a
peer's "do you have this object?" request without depending on
`canopee-storage` directly. `Runtime::open` supplies the concrete
implementation:

```rust
struct StorageObjectProvider { storage: Arc<Storage> }

impl ObjectProvider for StorageObjectProvider {
    async fn get_object(&self, id: &ObjectId) -> Option<ExportBundle> {
        let object = self.storage.get_verified(id).await.ok()?;
        object.export().ok()
    }
}
```

Only verified, exportable objects are ever served to peers — the same
verification path used for every other read.

## Node state

`Runtime` persists a small `NodeState` (identity, `created_at`,
`last_started_at`, `started`, `version`, `peers`) to
`~/.canopee/state/node.state`, written atomically (write to `.tmp`, then
`rename`) so a crash mid-write can't corrupt it. `mark_started`/
`mark_stopped` update it; [`canopee-node`](../canopee-node) calls
`mark_started` once its socket is bound and listening.

Note: `NodeState.peers` is currently unused — connected peers are tracked
live in [`canopee-network`](../canopee-network)'s `NetworkManager`, not
persisted here. This field is a placeholder for future peer-list
persistence (e.g. remembering known peers across restarts).

| Field | Purpose |
|---|---|
| `config` | The resolved `~/.canopee` paths ([`canopee-config`](../canopee-config)) |
| `identity` | `Arc<Identity>` — shared with the network layer, which needs the same keypair |
| `storage` | `Arc<Storage>` — shared with `StorageObjectProvider` |
| `network` | `NetworkManager` handle — public, so callers can drive the swarm directly |

## Design notes

- `identity` and `storage` are `Arc`-wrapped specifically so
  `NetworkManager::new` and `StorageObjectProvider` can each hold their own
  reference without `Runtime` needing unsafe aliasing or a lock.
- `Runtime` has no shutdown/stop method of its own beyond `mark_stopped` —
  process lifecycle (accepting connections, handling `Shutdown`, exiting)
  is owned by [`canopee-node`](../canopee-node), which holds the `Runtime`
  and decides when to stop using it.
- `import` refuses to overwrite an existing object (`anyhow::bail!` if the
  ID already exists) rather than silently no-op'ing or overwriting — since
  `ObjectId` is content-derived, this only ever happens when the incoming
  object is byte-for-byte identical to one already stored.

## Testing

No dedicated test suite in this crate; its behavior is exercised indirectly
through [`canopee-node`](../canopee-node)'s and
[`canopee-sdk`](../canopee-sdk)'s tests, which construct a real `Runtime`.
