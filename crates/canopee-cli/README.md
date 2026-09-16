# canopee-cli

The `canopee` command-line tool: a thin wrapper over
[`canopee-sdk`](../canopee-sdk) (and, for a couple of commands, its
lower-level `NodeClient`/[`canopee-protocol`](../canopee-protocol) directly)
for driving a [Canopee node](../canopee-node) from a shell.

## Commands

```bash
canopee init                              # generate identity + storage on disk (no running node needed)
canopee start                             # spawn `canopee-node` in the background
canopee stop                              # ask the running node to shut down
canopee status                            # identity, object count, peer count

canopee identity                          # this node's identity
canopee put <path>                        # store a file's contents as a signed object ("Stored <name>")
canopee get <name|id> [--output <file>]   # print a stored object's bytes (or write them to a file)
canopee desc <name|id>                    # object metadata + a content preview
canopee list                              # list your stored objects by name
canopee list --ids                        # idem, but also show the 64-char content-addressed ids
canopee export <object-id>                # write a portable .canopee bundle to ~/.canopee/exports
canopee import <path>                     # import a .canopee bundle into local storage

canopee dial <multiaddr>                  # connect directly to a peer, e.g. /ip4/1.2.3.4/tcp/4001/p2p/<id>
canopee listen-via-relay <relay-multiaddr> # request a relay circuit reservation (enables hole punching)
canopee peers                             # list connected peers by friendly name (profile display name / claimed
                                          # username) when resolvable, falling back to a shortened peer id
canopee relay-status                      # confirm accepted relay reservations + dialable circuit addresses
canopee announce <name|id>                # announce on the DHT that this node provides an object
canopee find-providers <name|id>          # list peers providing an object
canopee publish <topic> <message>         # publish a gossipsub message
canopee chat <topic>                      # interactive send/receive REPL on a gossipsub topic (senders shown by name)
canopee fetch <peer|username> <name|id>   # fetch an object from a peer — by shared *name*, or raw id
canopee share <name> [<id>]               # share an object by name (id defaults to your stored object named <name>)
canopee unshare <name>                    # stop sharing a home entry
canopee home                              # your catalog: entries, shared flags, apps

canopee username claim <name>             # claim a globally unique username (DHT registry + signed record)
canopee username show                     # show this node's claimed username
canopee username lookup <name>            # reverse-resolve a username to its owner identity (verified)

canopee app-manifest <dir> <name>         # publish a directory (must contain index.html) as an app manifest;
                                           # announces the manifest + every asset and publishes a signed (owner, name) pointer
canopee app-info <manifest-id>            # decode and print a locally-stored app manifest
canopee open <manifest-id> [--peer <id>] [--port <port>]           # fetch a manifest by id and serve it over HTTP
canopee open --owner <id> --name <name> [--peer <id>] [--port <port>] # or resolve the latest manifest via its (owner, name) pointer
```

Every command except `init` and `start` talks to an already-running node —
run `canopee start` (or `cargo run -p canopee-node` directly) first.

Object ids are hidden by default: storage, sharing and fetching work by human
names alone. Pass `--ids` (a global flag, valid on any subcommand) to see the
underlying 64-char content-addressed ids. A `<name|id>` argument with exactly
64 hex characters is treated as an id; anything else is resolved by name —
first against your stored file names (`put`/`fetch`), then against your home
entries (`share`).

## What each command does

| Command | Path |
|---|---|
| `init` | Opens a `Runtime` directly (no node required) just to trigger identity/storage creation and print the identity |
| `start` | Spawns `cargo run -p canopee-node` as a detached child process |
| `stop`, `status`, `identity`, `export`, `import`, `dial`, `listen-via-relay`, `peers`, `relay-status`, `publish`, `username claim/show/lookup` | Send one `NodeCommand` to the running node via `canopee_sdk::NodeClient` and print the `NodeResponse` |
| `put`, `get`, `desc`, `list`, `announce`, `find-providers` | Resolve any `<name|id>` arguments to ids by name first, then issue the `NodeCommand`. `put` prints `Stored <name>` — nothing but the name unless `--ids` is given. `list` shows named objects only (unnamed internals appear under `--ids`) |
| `share`, `unshare`, `home` | Via `canopee_sdk::CanopeeClient` (`share_object`/`set_home_entry_shared`/`load_home_index`). Sharing publishes the object's `(owner, "entry:<name>")` pointer, which is what `fetch <peer> <name>` resolves |
| `fetch` | `Get` the id first (a 64-char arg passes straight through, a name resolves `(owner, "entry:<name>")` to a distant owner's shared entry), then dials the peer — by thumbprint peer id, canonical identity, or claimed username — fetches the object, imports it, and (for name fetches) records the name so it lists locally |
| `chat` | Uses `canopee_sdk::CanopeeClient` to `subscribe` (printing incoming messages on a background task) and `publish` (from stdin) on the same topic — a small persistent REPL, not a one-shot request/response |
| `app-manifest` | Walks a directory (via [`app::publish_directory`](src/app.rs)), stores every file as a `Blob` object plus one `AppManifest` object pointing at them, `announce`s the manifest and every asset, and `publish_app_pointer`s a signed `(owner, name)` pointer to the manifest |
| `app-info` | Local `Get` + decode of a stored `AppManifest` object |
| `open` | Resolves a manifest id — directly, or via `resolve_app_pointer` from `--owner`/`--name` — then fetches it and every asset (locally, or from `--peer`/DHT lookup, concurrently) via [`app::fetch_app`](src/app.rs), and serves it over local HTTP via [`app::serve`](src/app.rs) |

See [`docs/testing-chat-between-peers.md`](../../docs/testing-chat-between-peers.md)
for a full walkthrough of `chat`, `peers`, and `relay-status` together to
test two nodes talking to each other, on a LAN or through a relay. See
[`docs/app-manifests.md`](../../docs/app-manifests.md) for the full
publish → announce → fetch → open lifecycle of `app-manifest`/`app-info`/`open`,
and [`docs/usernames.md`](../../docs/usernames.md) for how claimed usernames
work and where they show up.

## Example session

Alice stores a file, shares it, and Bob fetches it by name — no ids anywhere:

```bash
$ canopee start
Canopee node started (pid 12345)

$ canopee identity
IdentityId("canopee://identity/12D3KooW...")

# --- Alice ---
$ canopee put ./summer-mix.mp3
Stored summer-mix.mp3

$ canopee list
Canopee Objects:

summer-mix.mp3
Owner: IdentityId("canopee://identity/12D3KooW...")
Size: 75024 bytes
Signature: ✓ valid

$ canopee list --ids
Canopee Objects:

summer-mix.mp3
Id: 77cf93089a00c60463e955a3bb77a2621c41269ef3caecd36c463df5e7b30d3c
Owner: IdentityId("canopee://identity/12D3KooW...")
Size: 75024 bytes
Signature: ✓ valid

$ canopee get summer-mix.mp3 --output ./copy.mp3
Wrote 75024 bytes to ./copy.mp3 (summer-mix.mp3)

$ canopee desc summer-mix.mp3
Name: summer-mix.mp3
Owner: canopee://identity/12D3KooW...
Type: Blob
Size: 75024 bytes
Created: 1789586796

$ canopee username claim alice
Claimed username "alice"

$ canopee share summer-mix.mp3
Shared "summer-mix.mp3"

# --- Bob ---
$ canopee fetch alice summer-mix.mp3
Fetched "summer-mix.mp3" from alice

$ canopee get summer-mix.mp3 --output ./summer-mix.mp3
Wrote 75024 bytes to ./summer-mix.mp3 (summer-mix.mp3)

$ canopee stop
Canopee node stopped
```

## Design notes

- Names are first-class, ids are behind `--ids`. A stored object's name is a
  *sidecar* (`. <object-id>.name` in the store), not part of the signed
  object — so content-addressed ids and the on-disk object format never
  change, old objects keep listing, and re-putting identical bytes still
  yields the same id. `get` resolves a name via the sidecar (and a `home`
  entry, for entries that were shared without a sidecar name). Names never
  cross the wire inside the object itself.
- Sharing an entry publishes a signed `(owner, "entry:<name>")` pointer record
  -> the object id (alongside the usual `(owner, "home")` index update). That
  pointer is what lets a peer `fetch <owner> <name>` by name without ever
  seeing an id. Only *sharing* publishes it, so private entries never leak
  into records; `unshare` stops serving the object (a stale pointer resolves
  but the fetch is refused). Fetching by name imports the object and records
  the name, so the fetched file lists and re-shares by name.
- `start` shells out to `cargo run -p canopee-node` rather than invoking the
  compiled `canopee-node` binary directly. That's convenient in this
  workspace during development, but means `canopee start` currently
  requires the source tree and a Rust toolchain to be present at runtime —
  not what you'd want for a distributed binary. A packaged release should
  point this at the sibling `canopee-node` executable instead.
- `init` bypasses the running node entirely (it opens a `Runtime` in-process
  just for this one call) — useful for provisioning identity/storage ahead
  of the first `start`, but it means `init` and a running node's `Runtime`
  are two separate `Runtime::open()` calls that happen to agree on-disk.

## Testing

No dedicated test suite for this crate; it's exercised manually and via the
underlying [`canopee-sdk`](../canopee-sdk) and
[`canopee-node`](../canopee-node) tests.
