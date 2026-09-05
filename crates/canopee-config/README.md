# canopee-config

The single source of truth for where a Canopee node's files live on disk.
Every other crate that touches the filesystem —
[`canopee-runtime`](../canopee-runtime),
[`canopee-sdk`](../canopee-sdk)'s `NodeClient` — goes through `Config`
instead of hardcoding paths, so there's exactly one place that defines the
`~/.canopee` layout.

## API

```rust
use canopee_config::Config;

let config = Config::new(); // rooted at `dirs::home_dir()/.canopee`

config.home_dir();          // ~/.canopee
config.identity_path();     // ~/.canopee/identity
config.storage_path();      // ~/.canopee/storage
config.records_path();      // ~/.canopee/records
config.aliases_path();      // ~/.canopee/aliases
config.cache_path();        // ~/.canopee/cache.cache
config.export_path();       // ~/.canopee/exports
config.state_path();        // ~/.canopee/state/node.state
config.node_socket_path();  // ~/.canopee/node.sock
config.listen_addr();       // "/ip4/0.0.0.0/tcp/0" — the libp2p listen multiaddr
```

`Config` doesn't create any of these paths itself — callers (mainly
[`canopee-runtime::Runtime::open`](../canopee-runtime)) are responsible for
`create_dir_all`-ing directories before using them.

## Split-root: "state is per-app, data is per-user"

`Config` actually holds **two roots**: the *app root* (runtime state, socket,
exports, listen address) and the *user root* (identity, storage, records,
aliases). The default `Config::new()` points both at `~/.canopee`; explicit
builders control them independently:

```rust
// Both roots at an explicit directory — the classic fully-isolated
// embedded app (or test) setup.
let config = Config::with_root("/var/lib/myapp/.canopee");

// App state moves to `app_root`, but identity/storage/records/aliases stay
// at the shared `~/.canopee` user root — so every app of the same person
// shares one identity and one object store.
let config = Config::new().with_app_root(app_root);

// Move only the user scope (rare; tests use `with_roots` to set both).
let config = Config::new().with_user_root(user_root);
let config = Config::with_roots(app_root, user_root);

// mDNS on/off (embedded apps sharing one identity disable it so two swarms
// don't re-announce the same PeerId over multicast).
let config = Config::new().with_mdns(false);
```

Paths split accordingly:

| Per-app (under `home_dir()` / app root) | Per-user (under `user_root()`) |
|---|---|
| `state/node.state`, `node.sock`, `exports`, cache | `identity`, `storage`, `records`, `aliases` |

## Design notes

- `home_dir()` panics via `.unwrap()` if `dirs::home_dir()` returns `None`
  (no `$HOME`/platform equivalent resolvable). This is intentional for now —
  a node has nowhere sensible to live without it — but it means tests and
  tools that spawn a node need `$HOME` set, even to a scratch directory.
- `listen_addr()` is currently a fixed string
  (`/ip4/0.0.0.0/tcp/0`, ephemeral port on all interfaces) rather than
  configurable. Changing the node's bind address today means editing this
  crate.
- There's no config *file* here — despite the name, this crate is purely
  path resolution. Runtime behavior (identity, state, network) is configured
  in code by the crates that consume these paths.
- The split-root model is built into the type: `with_root` keeps both roots
  equal (backward compatible), while `with_app_root` / `with_user_root` /
  `with_roots` move the two scopes independently. The path getters choose
  root vs. user root accordingly, so callers never need to know which scope
  a given path belongs to.

## Testing

`cargo test -p canopee-config` covers the split-root builders: backward
compatibility of `with_root`, per-app/user scope separation, and the
mDNS toggle.
