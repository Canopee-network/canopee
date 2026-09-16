# Chapter 4: Configuration and Path Resolution

## The Path Model

Canopee has a precise model for where files live on disk. This model is built into the type system — there is no config file, no YAML, no TOML configuration. Paths are derived from two roots:

1. **App root**: application-specific state (socket, exports, cache, runtime state)
2. **User root**: shared across applications (identity, object store, records, aliases)

The default user root is `~/.canopee`.

## Split-Root Architecture

The split-root model is the key architectural decision in Canopee's configuration:

### Fully Isolated Mode

```rust
Config::with_root(path)
```

Both the app root and user root point to the same directory. Everything — identity, storage, socket, state — lives under one tree. This is used for testing and fully isolated deployments.

### Embedded App Mode

```rust
let config = Config::new().with_app_root(app_dir);
```

`with_app_root` is a builder method on `Config`, not a constructor — start from `Config::new()` and chain. The app root is the application's own data directory. The user root defaults to `~/.canopee`. This means:

- **Per-app**: socket, exports, cache, state → under the app's directory
- **Shared**: identity, storage, records, aliases → under `~/.canopee`

Two applications by the same user share one identity, one object store, and one set of records. Each application has its own socket, its own cache, and its own runtime state.

### Custom Roots

```rust
let config = Config::new().with_user_root(user_dir);           // custom user root
let config = Config::new().with_app_root(app).with_user_root(user); // both custom
```

All of `with_app_root`, `with_user_root`, and `with_mdns` are builder methods that consume and return `self`. For testing or special deployments where you need full control over both roots.

## Path Getters

The `Config` struct provides typed accessors for every path:

| Getter | Resolves To | Root |
|--------|-------------|------|
| `home_dir()` | The app root | App |
| `user_root()` | The user root (`~/.canopee`) | User |
| `identity_path()` | `{user}/identity/` (directory; the runtime appends `identity.key`) | User |
| `storage_path()` | `{user}/storage/` | User |
| `records_path()` | `{user}/records/` | User |
| `aliases_path()` | `{user}/aliases` (a JSON file, not a directory) | User |
| `cache_path()` | `{app}/cache.cache` (a single cache-index file) | App |
| `export_path()` | `{app}/exports/` | App |
| `state_path()` | `{app}/state/node.state` (a file) | App |
| `node_socket_path()` | `{app}/node.sock` | App |

`Config::new()` (not the getter) panics if `$HOME` cannot be resolved. This is intentional — Canopee cannot function without a home directory, and tests must set `$HOME` to a scratch directory.

## Network Configuration

The listen address is fixed at `/ip4/0.0.0.0/tcp/0` (ephemeral port) by default. The port can be overridden via the `CANOPEE_LISTEN_PORT` environment variable:

```bash
export CANOPEE_LISTEN_PORT=4001  # useful for relay nodes that need a stable address
```

For relay nodes, a stable port is essential — peers need a known address to connect. The `deploy/` directory includes a systemd unit that sets this.

## mDNS Configuration

mDNS (multicast DNS) is used for automatic peer discovery on local networks. It is enabled by default.

However, when Canopee is embedded in an application that runs multiple instances (or when two applications share the same identity on one machine), mDNS must be disabled to avoid duplicate announcements:

```rust
let config = Config::new().with_mdns(false);
```

This is critical for the embedded use case. Two runtimes sharing one identity must not both announce the same `PeerId` on multicast — it would confuse discovery.

## The No-Config Philosophy

Canopee deliberately has no configuration file. The rationale:

- **Deterministic behavior**: every path, every default, every behavior is determined by the code and the two root directories
- **No config drift**: there is no file that can get out of sync with the code
- **Testability**: any test can set up a scratch directory and get a clean Canopee environment
- **Embeddability**: the application controls configuration through the `Config` struct, not through files on disk

Environment variables (`CANOPEE_LISTEN_PORT`, `CANOPEE_IDENTITY_PASS`) provide escape hatches for deployment-specific needs without introducing config files.

## Directory Structure

A typical `~/.canopee` directory looks like:

```
~/.canopee/                     (user root — shared across apps)
├── identity/
│   └── identity.key            # Ed25519 keypair (plaintext or encrypted)
├── storage/
│   ├── a1b2c3...               # Object files, named by ObjectId
│   ├── d4e5f6...
│   └── ...
├── records/
│   └── <hex(sha256(dht-key))>.record  # Cached mutable records
└── aliases                     # JSON file: username → IdentityId map

<app-data-dir>/                 (app root — per-application)
├── node.sock                   # Unix domain socket (while running)
├── state/
│   └── node.state              # NodeState (identity, timestamps, version)
├── exports/
│   └── *.canopee               # Exported bundles
└── cache.cache                 # LRU cache index (cached objects from peers
                                #  themselves live in the shared storage/)
```

This structure makes it trivial to inspect, debug, or clean up a Canopee installation — it's just files on disk.
