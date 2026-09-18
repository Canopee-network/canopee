# Browser gateway

`canopee gateway` exposes the node to a browser tab as a local WebSocket
bridge — the piece that lets a published SPA drive the Canopee SDK from
JavaScript instead of requiring a desktop app.

## Start the gateway

```bash
canopee gateway
#   Canopee gateway ready for this machine only:
#     Demo page: http://127.0.0.1:35239/
#     WebSocket: ws://127.0.0.1:35239/?token=...
```

A random port is chosen (`--port` to fix it). Access is restricted:

- loopback-bound (not reachable from other machines),
- session-token gated (printed on startup, required in the WebSocket URL),
- non-loopback `Origin`s rejected.

The printed demo page URL opens a ready-to-use bridge; the page injects the
token itself, so nothing needs configuring.

## From the browser tab

A tiny `client.js` helper (shipped in `crates/canopee-gateway/www/client.js`)
opens the WebSocket, sends JSON commands, and receives JSON responses —
the same request/response shape as `canopee-protocol`, surfaced as
JavaScript promises. A published SPA loaded from the demo page (or from
`canopee open`) can:

```js
const identity = await client.invoke("identity");
const objects  = await client.invoke("list");
await client.invoke("put", { data: btoa("hello from the browser") });
```

Pub/sub events arrive as pushed JSON frames on the same socket.

## When to use a gateway

- You're publishing a **static SPA** that needs to talk to Canopee from
  JavaScript — see [Publishing apps](publishing-apps.md).
- You want a **browser-based chat or dashboard** without writing Rust.
- You're prototyping and want to experiment with the SDK from a console.

When the backend *is* part of your app — not just a published site — use
the embedded [Tauri](tauri-quickstart.md) pattern instead: link
`canopee-runtime` directly and sidestep the gateway entirely.

## Security model

The gateway is a **local-only trust domain**:

- Binds `127.0.0.1` — a remote site cannot connect.
- Requires a fresh per-process session token in the URL.
- Rejects WebSocket handshakes whose `Origin` is not a loopback origin.

A published SPA running on the gateway's own demo page can therefore
drive the SDK; a page hosted on a remote site cannot.