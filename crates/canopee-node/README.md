# canopee-node

The Canopee node daemon: owns a [`Runtime`](../canopee-runtime) (identity +
storage + network) and exposes it to clients over a Unix domain socket using
[`canopee-protocol`](../canopee-protocol). This is both a library (`Node`)
and a binary (`canopee-node`) — the binary is a thin wrapper that opens a
`Node` and runs it.

## Running it

```bash
cargo run -p canopee-node
# Canopee node started
# Canopee node listening on "~/.canopee/node.sock"
```

It stays in the foreground until it receives a `Shutdown` command (typically
via `canopee-cli stop` or `CanopeeClient::shutdown`), or the process is
killed. There's no daemonization here — running it in the background is the
caller's responsibility (`&`, a supervisor, `canopee-cli start` spawns it via
`cargo run` today — see [`canopee-cli`](../canopee-cli)).

## What `Node::run` does

1. Removes any stale socket file at `node.sock`, then binds a fresh
   `UnixListener` there
2. Calls `runtime.mark_started()`
3. Loops: accept a connection → spawn a task to handle it (tracked in a
   `JoinSet`) → repeat, until a shutdown signal fires
4. On shutdown: aborts all in-flight connection tasks, removes the socket
   file, and returns

Each connection is handled independently and concurrently — a slow or
long-lived client (e.g. a pub/sub subscriber, which holds its connection
open indefinitely) never blocks other clients.

## Connection handling

```
client connects
  │
  ▼
read one NodeCommand
  │
  ├─ Subscribe { topic } ──► handle_subscribe: hijack the connection,
  │                          stream PubSub messages until disconnect
  │                          (see canopee-protocol's streaming-command note)
  │
  └─ anything else ─────────► handle(command) → one NodeResponse → done
```

`handle(command) -> NodeResponse` is the core dispatch: it matches every
`NodeCommand` variant to the corresponding `Runtime`/`NetworkManager` call
and wraps the result (or error) into a `NodeResponse`. It's exposed publicly
so it can be unit-tested without going through a real socket (see
`test_node_put`).

`Shutdown` is handled specially in `handle_connection`: after the response
is written, the node broadcasts a shutdown signal that unblocks the accept
loop in `run`.

## Design notes

- `Node` is `Clone` (an `Arc<Runtime>` plus a `broadcast::Sender<()>` for
  shutdown) specifically so each spawned connection task can hold its own
  cheap handle without borrowing issues.
- Every command and response is logged to stdout (`println!`) as it's
  processed — useful for development, noisy for anything else. There's no
  structured logging or log-level control yet.
- If writing a response fails (client disconnected mid-write), the node
  logs it and moves on rather than treating it as fatal — one flaky client
  shouldn't take down the node.
- `Shutdown`'s response is written *before* the shutdown signal is sent, so
  the client requesting shutdown reliably gets its `ShutdownAccepted` before
  the node starts tearing down.

## Testing

```bash
cargo test -p canopee-node
```

The one test (`test_node_put`) exercises `Node::handle` directly against a
real `Runtime::open()` — no socket involved — which is enough to catch
wiring mistakes between `NodeCommand` and the runtime's methods without the
overhead of a full client/server round trip.

For full end-to-end coverage of the socket protocol (including the
streaming `Subscribe` path), see
[`canopee-sdk`](../canopee-sdk)'s integration tests, which run a real
`Node` and drive it purely through `CanopeeClient`.
