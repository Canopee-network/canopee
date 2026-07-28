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
canopee put <path>                        # store a file's contents as a signed object
canopee get <object-id>                   # print a stored object's id
canopee list                              # list stored objects with verification status
canopee export <object-id>                # write a portable .canopee bundle to ~/.canopee/exports
canopee import <path>                     # import a .canopee bundle into local storage

canopee dial <multiaddr>                  # connect directly to a peer, e.g. /ip4/1.2.3.4/tcp/4001/p2p/<id>
canopee listen-via-relay <relay-multiaddr> # request a relay circuit reservation (enables hole punching)
canopee peers                             # list currently connected peers
canopee relay-status                      # confirm accepted relay reservations + dialable circuit addresses
canopee publish <topic> <message>         # publish a gossipsub message
canopee chat <topic>                      # interactive send/receive REPL on a gossipsub topic
```

Every command except `init` and `start` talks to an already-running node —
run `canopee start` (or `cargo run -p canopee-node` directly) first.

## What each command does

| Command | Path |
|---|---|
| `init` | Opens a `Runtime` directly (no node required) just to trigger identity/storage creation and print the identity |
| `start` | Spawns `cargo run -p canopee-node` as a detached child process |
| `stop`, `status`, `identity`, `put`, `get`, `list`, `export`, `import`, `dial`, `listen-via-relay`, `peers`, `relay-status`, `publish` | Send one `NodeCommand` to the running node via `canopee_sdk::NodeClient` and print the `NodeResponse` |
| `chat` | Uses `canopee_sdk::CanopeeClient` to `subscribe` (printing incoming messages on a background task) and `publish` (from stdin) on the same topic — a small persistent REPL, not a one-shot request/response |

See [`docs/testing-chat-between-peers.md`](../../docs/testing-chat-between-peers.md)
for a full walkthrough of `chat`, `peers`, and `relay-status` together to
test two nodes talking to each other, on a LAN or through a relay.

## Example session

```bash
$ canopee start
Canopee node started (pid 12345)

$ canopee identity
IdentityId("canopee://identity/12D3KooW...")

$ canopee put ./notes.txt
Created object:
3041bc1432545ec0a867f63a4e538886e19faede25e4f1e138434d5ab4aa4882

$ canopee list
Canopee Objects:

3041bc1432545ec0a867f63a4e538886e19faede25e4f1e138434d5ab4aa4882
Owner: IdentityId("canopee://identity/12D3KooW...")
Size: 42 bytes
Signature: ✓ valid

$ canopee status

Canopee Node

Running:
yes

Identity:
canopee://identity/12D3KooW...

Objects:
1

Peers:
0

$ canopee stop
Canopee node stopped
```

## Design notes

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
- Most commands mostly print the raw `Debug` form of protocol responses
  (`{:?}`) rather than nicely formatted output — this is a developer tool
  first, not a polished end-user CLI yet.

## Testing

No dedicated test suite for this crate; it's exercised manually and via the
underlying [`canopee-sdk`](../canopee-sdk) and
[`canopee-node`](../canopee-node) tests.
