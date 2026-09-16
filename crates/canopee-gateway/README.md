# canopee-gateway

A loopback-only bridge that lets a **browser tab** drive a running Canopee
node. [canopee-sdk](../canopee-sdk) talks to the node over a Unix socket,
which a browser sandbox can't reach — the gateway puts a small WebSocket
server on `127.0.0.1` in front of that socket, and translates each JSON
command from the browser into a `CanopeeClient` call against the local node.

```
browser tab ── WebSocket (JSON) ──> canopee-gateway ── Unix socket (bincode) ──> canopee-node
```

This is the bridge that makes a *published* static app into a real network
citizen — see [`docs/publishing-vs-building-apps.md`](../../docs/publishing-vs-building-apps.md)
for the publish-vs-build distinction, and
[`docs/gateway-tutorial.md`](../../docs/gateway-tutorial.md) for the
hands-on walkthrough.

## Quick start

```bash
cargo run -p canopee-cli -- gateway
```

prints a demo-page URL and a WebSocket URL (both on `127.0.0.1`). Open the
demo page in a browser — it shows your identity, lets you `put`/`get`
objects, and runs a pub/sub chat — or point your own app at the WebSocket
with the included client:

```js
const c = new CanopeeWeb("ws://127.0.0.1:PORT/?token=…");
await c.connect();
const id = await c.put("hello from the tab");
const obj = await c.get(id);
```

The browser client is [`www/client.js`](www/client.js): a standalone,
no-build, promise-based `CanopeeWeb` class mirroring `CanopeeClient`. The
same client is embedded in the gateway's demo page and copied (for
publishing) into the example at
[`portfolio-test/gateway-demo/`](../../portfolio-test/gateway-demo/).

## Trust model

The gateway is a strictly local trust domain:

- it binds **loopback only** — a remote client can't reach it at all,
- every `Gateway` instance has a fresh 128-bit `SessionToken`; the browser
  must present it in the WebSocket URL's `?token=`, and the gateway injects
  its own token into the demo page server-side so it never has to be typed,
- handshakes whose `Origin` header isn't a loopback origin are refused
  (browsers always send `Origin` on WebSocket opens, so no other website can
  reach into your node),
- it exposes a **safe command subset**: no shutdown, no low-level
  administration, nothing broader than what `CanopeeClient` offers.

Bytes and ids are re-formatted for JSON (opaque ids as plain strings,
payloads as base64) in [`src/protocol.rs`](src/protocol.rs); the exact
translation to `CanopeeClient` calls is a one-to-one table in
[`src/lib.rs`](src/lib.rs). The WebSocket server itself is hand-rolled RFC
6455 in [`src/ws.rs`](src/ws.rs) — consistent with the repo's
no-http-framework stance (see `canopee-cli/src/app.rs`).

## Workspace role

The gateway never touches libp2p or the network itself — it's a pure
transport in front of `CanopeeClient`, the same way the CLI is. The node
remains the single source of truth and the only network participant.

## Testing

```bash
cargo test -p canopee-gateway
```

Unit tests cover the wire types and the WebSocket framing (including the RFC
6455 `accept`-key vector). `tests/gateway.rs` boots a real in-process `Node`
on a scratch `$HOME`, starts a gateway in front of it, and drives it through
a hand-rolled WebSocket *client*: demo page serving, token/origin rejection,
the command set, and the streaming-subscription path.