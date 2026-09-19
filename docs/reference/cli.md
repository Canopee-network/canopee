# CLI reference

Every command in the `canopee` (implemented in `crates/canopee-cli`) binary.
Binary is also invocable as `canopee-cli` when built from source.

Global options:

| Option | Effect |
|---|---|
| `--ids` | Print content-addressed object ids (hidden by default — this CLI is designed so storage, sharing and fetching work by human names alone) |
| `-h` / `--help` | Help |

Most commands talk to a running node over its Unix socket
(`node_socket_path`, see [Filesystem layout](filesystem.md)). `canopee init`
and `canopee start` are the exceptions (see below).

## Lifecycle & node control

| Command | What it does |
|---|---|
| `canopee init` | Initialize `~/.canopee`: identity, device key, storage, records, config. Safe to re-run. |
| `canopee start` | Start the daemon in the background. |
| `canopee stop` | Stop the running daemon. |
| `canopee status` | Report whether a node is running and basic info. |
| `canopee profile [NAME]` | With no argument, show the current profile (display name, version). With `NAME`, set the display name. |
| `canopee devices` | List the devices currently carrying this node's identity (from `(owner, "devices")`). |
| `canopee device` | Print this machine's device `PeerId` and the human-friendly device name it registers under its identity. |

## Object storage

| Command | What it does |
|---|---|
| `canopee list` | List your stored objects (name, type, size, …). |
| `canopee put <path>` | Store a file (or text) as an object locally. `--ids` shows the raw id. |
| `canopee get <id\|name>` | Retrieve an object by id or stored name. `--output <file>` writes bytes to a file; otherwise prints to stdout (as UTF-8 text when possible). |
| `canopee desc <id\|name>` | Show metadata (owner, type, size, name) plus a preview so you can tell what a stored name actually is. |
| `canopee export <id>` | Export an object as a portable `ExportBundle` (stdout/file). |
| `canopee import <path>` | Import an `ExportBundle`. |
| `canopee home` | List the entries in your home index (name, object id, shared flag). |

## Network

| Command | What it does |
|---|---|
| `canopee dial <addr>` | Dial a multiaddr directly. |
| `canopee listen-via-relay <relay-addr>` | Set up a circuit-relay reservation on `relay-addr` so the node is reachable without a direct connection. |
| `canopee peers` | List known/connected peers. |
| `canopee relay-status` | Report relay reservations / NAT state. |

## Sharing & fetching

| Command | What it does |
|---|---|
| `canopee share <name> [id]` | Share a stored object under a name: upsert `shared:true` in the home index, publish `(owner,"entry:<name>")` pointer, announce on DHT. `[id]` may be a raw id or a stored name; omit to share the object already named `<name>`. |
| `canopee unshare <name>` | Stop sharing: withdraw DHT provider announcement and stop serving the entry. Record removed from the public pointer set. |
| `canopee fetch <peer\|identity\|username> <id\|name>` | Fetch an object from a specific peer. `<id>` may be a raw 64-char hex id (direct fetch) or a name the peer shared it under (resolve `(owner,"entry:<name>")` first). `<peer>` may be a peer id, `canopee://identity/...`, or a username. |
| `canopee announce <id\|name>` | Announce on the DHT that this node provides the object (provider record). |
| `canopee find-providers <id\|name>` | List peer ids that have announced themselves as providers of the object. |
| `canopee sync [peer-id]` | Refresh user records (profile, contacts, devices) from the network. No argument: sync from every registered device. With a peer id: from that peer. |

## Pub/sub

| Command | What it does |
|---|---|
| `canopee publish <topic> <message>` | Publish a message to a gossipsub topic. |
| `canopee chat <topic>` | Interactive chat terminal on a topic (subscribe + publish loop). |

## Identity & pairing

| Command | What it does |
|---|---|
| `canopee export-identity --passphrase <P>` `[--output <path>]` | Export the identity key as an encrypted file for transfer to another device. Default output `identity-export.bin`. |
| `canopee import-identity <path> --passphrase <P>` `[--overwrite]` | Import a previously exported identity key. Written to disk; **restart the node to adopt it**. With `--overwrite`, the existing identity is backed up before replacement. |
| `canopee pair [qr] [--code <CODE>]` | Pair another LAN device into the same identity. On the **new** device run `canopee pair` (prints a 12-char code + QR payload). On the existing device run `canopee pair <qr> --code <code>` — typing the code is explicit approval; the code never travels over the wire. |

## Usernames

| Command | What it does |
|---|---|
| `canopee username claim <name>` | Claim a globally unique username for this node's identity. |
| `canopee username show` | Show the username currently claimed by this node's identity. |
| `canopee username lookup <name>` | Reverse-resolve a username to its canonical `canopee://identity/<peer-id>` owner. |

## Capabilities

`canopee cap` — signed grants of permission over resources. Full detail in
the [Capabilities reference](capabilities.md).

| Command | What it does |
|---|---|
| `canopee cap grant <subject> <resource>` `[--read/--write/--publish]` `[--expires <secs>]` | Issue a capability. `<subject>` is `canopee://identity/...`; `<resource>` is `object:<id>`, `channel:<topic>`, or `shared:<owner>:<name>`. At least one permission flag required; default no expiry. |
| `canopee cap list` | List every capability this node has issued, with revocation state. |
| `canopee cap revoke <id>` | Revoke a capability by its (content-derived) id. |
| `canopee cap check [bundle]` `[--subject S --permission P --resource R]` | Verify a capability: pass a base64 `ExportCapability` bundle, or check whether `--subject` currently holds an unrevoked, unexpired `--permission` grant on `--resource` from this node's identity. |

## Apps & URI handling

| Command | What it does |
|---|---|
| `canopee app-manifest <dir> <name>` | Build an `ApplicationManifest` from a static directory + app name and publish it (pointer `(owner,"app:<name>")`). |
| `canopee publish <dir>` | Publish a static site to the public web through an edge: uploads the directory (as `app-manifest` does, under `app:<dirname>`), then runs a foreground serve session so the site is live at `https://<app-manifest-hash>.<CANOPEE_PUBLIC_BASE_DOMAIN>/` until Ctrl+C. No username needed — the URL is a hash of the manifest, so only the owner can serve it; republishing changed content yields a new URL. Defaults to the built-in public edge (the bootstrap relay); `CANOPEE_EDGE_ADDR` overrides. |
| `canopee app-info <id\|name>` | Show metadata about a published app manifest. |
| `canopee open <id\|name>` `[--owner OWNER --name NAME]` | Open a stored object with the system's default app (bytes → temp file → OS opener), or open a *published app*: resolve the manifest (raw id, or `--owner`/`--name`), fetch it, serve it over local HTTP, launch the browser. |
| `canopee handle <uri>` `[--port N]` | Resolve a `canopee://` URI (e.g. `canopee://alice/portfolio`) to an app and serve it in the default browser. Used by the OS scheme handler. |
| `canopee alias set <name> <owner>` | Map a friendly name (e.g. `alice`) to a canonical `canopee://identity/<peer-id>` owner inside `canopee://` URIs. |
| `canopee alias list` | List known aliases. |
| `canopee alias remove <name>` | Remove an alias. |
| `canopee uri-register` / `uri-unregister` | Register/remove the OS-level `canopee://` scheme handler routing to `canopee handle`. |

## Gateway

| Command | What it does |
|---|---|
| `canopee gateway [--port N]` | Expose the node to a browser tab as a local WebSocket gateway. Prints the demo URL, then serves `http://127.0.0.1:<port>/` (demo page) and `ws://127.0.0.1:<port>/?token=...` (the WebSocket bridge that `crates/canopee-gateway/www/client.js` speaks). Loopback-only, session-token-gated, non-loopback Origins rejected. |

## Message bus / platform

| Command | What it does |
|---|---|
| `canopee chat <topic>` | Interactive topic chat (see Pub/sub). |
| `canopee status` | Node status. |

### Command → SDK mapping

For developers: every CLI command corresponds to a `CanopeeClient` method.
A complete machine-checkable listing is in the [SDK reference](sdk.md).