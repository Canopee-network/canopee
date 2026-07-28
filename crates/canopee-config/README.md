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
config.export_path();       // ~/.canopee/exports
config.state_path();        // ~/.canopee/state/node.state
config.node_socket_path();  // ~/.canopee/node.sock
config.listen_addr();       // "/ip4/0.0.0.0/tcp/0" — the libp2p listen multiaddr
```

`Config` doesn't create any of these paths itself — callers (mainly
[`canopee-runtime::Runtime::open`](../canopee-runtime)) are responsible for
`create_dir_all`-ing directories before using them.

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

## Testing

No behavior to test beyond path composition; there is no test suite for
this crate.
