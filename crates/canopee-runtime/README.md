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
2. Generates and persists a new identity (or loads the existing one — see
   `Identity::create_if_absent` below)
3. Loads or initializes `NodeState` (identity, timestamps, started flag)
4. Opens the object [`Storage`](../canopee-storage)
5. Loads the persisted `HomeIndex`'s `shared: true` entries into the serving
   set (so a share survives restart)
6. Starts the [`NetworkManager`](../canopee-network), wired to a
   `StorageObjectProvider` so peers can fetch objects this node holds

Everything after that is a method call on `Runtime`: `put`, `get`, `list`,
`export`/`import`, plus direct access to `runtime.network` for the
peer-to-peer operations (`dial`, `announce`, `find_providers`, `get_object`,
`publish`, `subscribe`, ...).

### Split-root constructors

```rust
// Classic single-root: identity/storage/socket all under the given path.
// Two runtimes with different roots are fully isolated.
let a = Runtime::open_with_root(root_a).await?;

// Embedded desktop app: app state (socket, exports, cache) under `app_root`,
// identity/storage/records stay at the shared `~/.canopee` user root.
// Two such apps share one identity and one object store.
let b = Runtime::open_with_user_root(app_root).await?;

// Fully explicit (tests).
let c = Runtime::open_with_config(Config::with_roots(app, user)).await?;
```

The split-root model ("state is per-app, data is per-user") is documented in
[`canopee-config`](../canopee-config/README.md); the user-sharing runtime
test (`shared_user_root_gives_one_identity_and_shared_store`) proves two
runtimes with a shared user root resolve to one identity and one shared
store.

### User records

`Runtime` owns the user-record layer (the mutable pointers over immutable
objects) end-to-end:

```rust
// Generic pointer: (owner, name) -> ObjectId, signed.
runtime.publish_pointer("app:demo", id).await?;
let record = runtime.resolve_pointer(owner, "app:demo").await?;   // None if not found/unverifiable

// Profile: server-side version bump, (owner, "profile").
let id = runtime.save_profile(&profile).await?;
let profile = runtime.load_profile().await?;   // None until saved

// Contact list: (owner, "contacts").
runtime.save_contact_list(&list).await?;
let list = runtime.load_contact_list().await?;

// Home index: (owner, "home"), the user's "my data" table of contents.
runtime.save_home_index(&index).await?;
let index = runtime.load_home_index().await?;
```

Publishing is cache-first (the local `records/` directory is the
authoritative same-machine view, written atomically) with the DHT put fired
in the background; resolving checks the local cache first, then the DHT
bounded at `RESOLVE_DHT_TIMEOUT` (10s) so an unreachable network never
stalls a local lookup.

### Sharing gate ("nothing shared by default")

`StorageObjectProvider` only serves an object to the network when it's
**shared** — either because it was `announce`d (the explicit network action,
including the CLI's app-publish path) or because it's listed in a
`HomeIndex` entry with `shared: true`. Cached objects (imported from a peer)
are always re-served — that re-hosting *is* the distributed cache.

```rust
// The CLI `share` command, or "publish an app" path:
runtime.announce(id).await?;          // marks + announces
runtime.share_object("pic.png", &id, Some("photos")).await?;
                                    // upsert a shared home entry too

// Stop sharing:
runtime.set_home_entry_shared("pic.png", false).await?;

// Look:
runtime.is_shared(&id).await;         // shared or cached?
```

`save_home_index` reconciles the serving set: newly `shared: true` entries
are announced, `false` entries are unannounced (both best-effort, in the
background — the local shared set gates serving regardless of network
health).

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
| `identity` | `Arc<Identity>` — the shared *account* key; signs shared objects and user records |
| `device_key` | `Arc<DeviceKey>` — this machine's keypair; its public key is the network `PeerId` |
| `storage` | `Arc<Storage>` — shared with `StorageObjectProvider` |
| `network` | `NetworkManager` handle — public, so callers can drive the swarm directly |

## Design notes

- `identity` and `storage` are `Arc`-wrapped so shared references can be
  handed around without `Runtime` needing unsafe aliasing or a lock:
  `storage` is passed to `StorageObjectProvider`, and the *device*
  keypair (`device_key.keypair()` — not the account key) is what
  `NetworkManager::new` builds the swarm with.
- `Runtime` has no shutdown/stop method of its own beyond `mark_stopped` —
  process lifecycle (accepting connections, handling `Shutdown`, exiting)
  is owned by [`canopee-node`](../canopee-node), which holds the `Runtime`
  and decides when to stop using it.
- `import` refuses to overwrite an existing object (`anyhow::bail!` if the
  ID already exists) rather than silently no-op'ing or overwriting — since
  `ObjectId` is content-derived, this only ever happens when the incoming
  object is byte-for-byte identical to one already stored.
- `Identity::create_if_absent` is what makes the **shared user root race-
  safe**: whichever app creates the key first wins, everyone else loads it.
  A shared user root is only safe for `create`, never for two live
  `Identity::load`s racing each other, which is why both the
  `open_with_user_root` constructor and the CLI never call `load` directly.
- DHT operations (`put_record`, `announce`, `unannounce`) are fired in the
  **background** rather than awaited — a kad query is allowed to take its
  full ~60s timeout, and that latency must never stall a local publish or
  save. The local record cache remains the authoritative same-machine view;
  only the network publish is fire-and-forget.
- `RESOLVE_DHT_TIMEOUT` (10s) bounds first-run pointer resolution on a slow
  or unreachable network; healthy local lookups answer in milliseconds.

## Testing

`cargo test -p canopee-runtime` covers the split-root constructors (isolated
roots, shared user root convergence), user-record CRUD with version bumping,
the **sharing gate** (owned-unshared refused, shared served, cached always
served), home-index reconcile on save/load, share persistence across
reopen, and a two-node network test proving private objects are refused
over the wire while announced ones are fetched via DHT and re-served as
cache. The `ObjectProvider` behavior is exercised in-process through
`provider_for` helpers rather than needing a live swarm.
