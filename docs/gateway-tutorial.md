# Tutorial: driving Canopee from a browser with `canopee gateway`

Canopee's node listens on a Unix socket, which a browser tab can't reach.
This tutorial wires the two together: `canopee gateway` puts a small
loopback WebSocket server in front of the node, browser JavaScript drives
the SDK over that socket, and a published app (see
[`publishing-vs-building-apps.md`](publishing-vs-building-apps.md) for the
publish-vs-build distinction) becomes a full network citizen — storing,
pub/sub, app pointers, sharing — straight from the tab.

Read [`app-manifests.md`](app-manifests.md) and
[`publishing-vs-building-apps.md`](publishing-vs-building-apps.md) first if
you haven't; this tutorial assumes you know what a node, an identity, and a
published static app are.

## What a gateway is (and is not)

Conceptually:

```
browser tab ── WebSocket (JSON) ──> canopee-gateway ── Unix socket (bincode) ──> canopee-node
    canopee-gateway/src/protocol.rs                        canopee-sdk/CanopeeClient
```

`canopee-gateway` ([`crates/canopee-gateway`](../crates/canopee-gateway)) is
a thin transport: one connection to the gateway maps to one
[`CanopeeClient`](../crates/canopee-sdk/README.md) connection to the node.
Every JSON command from the browser becomes a `CanopeeClient` call, and
every `NodeResponse` comes back as a JSON event. Addresses and bytes are
re-formatted for JSON (opaque ids stay strings, payloads become base64),
but nothing else is interpreted — the gateway doesn't filter content, cache
objects, or invent behavior. It is not a peer and it holds no state; the
node remains the single source of truth.

Because the *gateway always runs on the same machine as the node*, it can
be a strict trust domain:

- it binds `127.0.0.1` only — no remote client can even reach it,
- every process generates a fresh 128-bit `SessionToken` that clients
  must present in the WebSocket URL's `?token=…` query string,
- handshakes whose `Origin` header isn't a loopback origin are refused
  (browsers always send `Origin` on WebSocket opens, so no *other* website
  can reach into your node),
- it exposes a *safe command subset*: there is no shutdown, and no
  low-level administration. The [`gateway-demo`](../portfolio-test/gateway-demo/index.html)
  example page relies on exactly this: the token is pasted in by hand, and
  the gateway injects it into its own demo page so visitors never even see
  it.

## Quick start

With a node running locally:

```bash
canopee gateway
```

which prints:

```text
Canopee gateway ready for this machine only:
  Demo page: http://127.0.0.1:36271/
  WebSocket: ws://127.0.0.1:36271/?token=0f2c5a1e…
```

The demo page is a working app: it shows your identity, lets you `put`/`get`
objects, and runs a pub/sub chat on topic `gateway-demo`. Open it and click
around — no configuration, because the token is injected into the page
server-side. To pick a fixed port: `canopee gateway --port 8080`.

## Using the browser client in your own app

The client lives at
[`crates/canopee-gateway/www/client.js`](../crates/canopee-gateway/www/client.js)
— a standalone, no-build, promise-based `CanopeeWeb` class. Copy it next to
your app:

```bash
cp crates/canopee-gateway/www/client.js ./canopee-client.js
```

Create a page (this is [`portfolio-test/gateway-demo`](../portfolio-test/gateway-demo/index.html)
in this repo, ready to `canopee app-manifest`):

```html
<script src="canopee-client.js"></script>
<script>
  const canopee = new CanopeeWeb("ws://127.0.0.1:36271/?token=0f2c5a1e…");
  await canopee.connect();

  const identity = await canopee.identity();          // "canopee://identity/12D3KooW…"
  const id = await canopee.put("hello from the tab"); // content-addressed object id
  const object = await canopee.get(id);               // { id, owner, text, dataB64, ... }
  const objects = await canopee.list();               // all stored objects

  canopee.subscribe("chat", (msg) => console.log(`[${msg.topic}] ${msg.text}`));
  await canopee.publish("chat", "hi, network");
</script>
```

`publish` is fire-and-forget from the gateway's perspective (the SDK's
`socket.put(topic, data)`), and `subscribe` returns immediately; messages
then stream in as `pubSub` events handled by your callback. Every *other*
method is a request/response: the promise resolves with the gateway's `ok`
result or rejects with the gateway's `error` message. Requests are answered
in the order they were sent, so the client matches them FIFO.

Publishing a page like this works exactly like any static app, except that
the page now has a live bridge to the node:

```bash
canopee app-manifest ./gateway-demo my-gateway-app
canopee open --owner <your-identity> --name my-gateway-app
```

The people who *use* your app need a node running and paste in the
gateway's WebSocket URL (as the example page's "connect" box does) — the
token is a secret that only they can click into their own node.

## Wire protocol reference

Commands are JSON objects tagged with `op` (camelCase); the gateway answers
every command except `subscribe` with a tagged `GatewayEvent`:

```jsonc
// request                          // response
{ "op": "identity" }                    → {"op":"ok","result":{"identity":"canopee://identity/…"}}
{ "op": "put", "text": "..." }          → {"op":"ok","result":{"id":"…"}}
{ "op": "get", "id": "…" }              → {"op":"ok","result":{"object":{"id":…,"owner":…,"text":…}}}
{ "op": "list" }                        → {"op":"ok","result":{"objects":[{ "id":…,"owner":…,"size":…,"verified":… }]}}
{ "op": "publish", "topic": "t", "text": "x" }  → {"op":"ok",…} or {"op":"error","message":"…"}
{ "op": "subscribe", "topic": "t" }             → (streaming) {"op":"pubSub","topic":"t","source":"…","text":"…","dataB64":"…"}
```

Full `GatewayCommand`/`GatewayEvent` definitions — including the view types
that keep ids as strings — are in
[`crates/canopee-gateway/src/protocol.rs`](../crates/canopee-gateway/src/protocol.rs).
The mapping to `CanopeeClient` calls is a one-to-one table in
[`crates/canopee-gateway/src/lib.rs`](../crates/canopee-gateway/src/lib.rs)
(`handle`): every JSON field either flows straight through or is re-wrapped
as the SDK expects.

The three JSON keys that differ from the Rust types, by design:

| JSON field | Rust type | Why it's re-formatted |
|---|---|---|
| `"id"`, `"owner"`, `"peerId"` | `ObjectId`, `IdentityId`, `PeerId` | wrapper types serialize as `{"0": …}` in serde — the gateway flattens them to plain strings |
| `"dataB64"` | `Vec<u8>` | bytes aren't valid JSON; base64 is the browser-native encoding |
| `"text"` | `Vec<u8>` (put/publish input) | asymmetric convenience: `put(…)`/`publish(…)`/`get(…)` accept and return UTF-8 text for the common case; `dataB64` stays for binary |

## What's deliberately excluded

The gateway intentionally does *not* expose the node's shutdown, or any
other low-level administration. Just as importantly, it has no session
management or authorization beyond the token — a peer that somehow obtained
your token also gets your `put`/`get`/`share`, so guard the token the way
you'd guard a `node-client` socket. Everything the gateway can do, the SDK
can do; the point is only to make those calls reachable from browser
JavaScript on the same machine. Code lives in
[`crates/canopee-gateway/src/ws.rs`](../crates/canopee-gateway/src/ws.rs) —
a hand-rolled RFC 6455 server (consistent with the repo's no-http-framework
stance, see `canopee-cli/src/app.rs`).

## Not covered here

- Deciding whether the browser or a backend should talk to the network at
  all → [`publishing-vs-building-apps.md`](publishing-vs-building-apps.md).
- Tauri desktop apps, which embed the SDK and skip the gateway entirely →
  [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md).
- The underlying node protocol the gateway translates →
  [`crates/canopee-sdk/README.md`](../crates/canopee-sdk/README.md) and
  [`crates/canopee-protocol/README.md`](../crates/canopee-protocol/README.md).