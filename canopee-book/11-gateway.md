# Chapter 11: The Gateway

## Browser Bridge

The gateway is a WebSocket-to-Unix-socket bridge that allows browser applications to communicate with the Canopee node. Browsers cannot open Unix sockets, but they can open WebSocket connections — and a loopback WebSocket is a safe bridge.

```
Browser Tab → WebSocket (127.0.0.1) → Gateway → Unix Socket → Node
```

## Architecture

The gateway is a thin, stateless relay. It translates between two protocols:

- **Browser side**: JSON over WebSocket (RFC 6455)
- **Node side**: bincode over Unix domain socket

The gateway adds no business logic. It translates, relays, and translates back. The browser application talks to the node as if it were a direct client.

## Session Security

Each gateway session gets a random token:

```
SessionToken = random_hex(16)  // from /dev/urandom
```

The token is embedded in the served HTML page and included in every WebSocket connection. The gateway verifies:

1. The token matches the current session
2. The `Origin` header is loopback-only (no cross-origin requests)

The token provides CSRF-like protection for the loopback connection. Since the gateway only binds to `127.0.0.1`, external pages cannot connect.

## WebSocket Protocol

The gateway uses JSON messages tagged by an `"op"` string. There is no request `id` and no `ok` boolean envelope — responses are matched to requests in FIFO order.

### Client → Gateway (JSON)

Commands are tagged by `"op"`. For example, `put` takes UTF-8 `text`:

```json
{ "op": "put", "text": "hello" }
```

```json
{ "op": "resolvePointer", "owner": "canopee://identity/...", "name": "my-app" }
```

### Gateway → Client (JSON)

Events are also tagged by `"op"`. A successful command returns `ok` with a `result` payload:

```json
{ "op": "ok", "result": { "id": "a1b2c3..." } }
```

A failure returns `error` with a `message`:

```json
{ "op": "error", "message": "Canopee node is not running" }
```

The client matches each `ok`/`error` to the oldest pending request (FIFO).

### Pub/Sub Stream

For subscribed connections, the gateway pushes `pubSub` events:

```json
{
  "op": "pubSub",
  "topic": "chat-room",
  "source": "12D3KooWABCD...",
  "text": "Hello",
  "dataB64": "SGVsbG8="
}
```

## Available Commands

The gateway exposes the node commands considered safe for a web page. It deliberately excludes `Shutdown`, `Status`, `RelayReservations`, `PutObject`, and `ListenViaRelay`:

- `identity`, `put`, `get`, `list`, `export`, `import`
- `peers`, `dial`, `announce`, `findProviders`, `fetchObject`
- `publishAppPointer`, `resolveAppPointer`
- `publishPointer`, `resolvePointer`
- `saveProfile`, `loadProfile`, `saveContactList`, `loadContactList`
- `saveHomeIndex`, `loadHomeIndex`, `setHomeEntryShared`, `shareObject`
- `claimUsername`, `showUsername`, `resolveUsername`
- `publish`, `subscribe`

The `Shutdown` command is deliberately excluded — a web page should never be able to shut down the node.

## The Demo Page

When you visit `http://127.0.0.1:PORT/`, the gateway serves a self-contained demo page:

1. The gateway generates a session token
2. It injects the token and the bundled `client.js` into the HTML template
3. The page loads with a working chat + storage UI

The user never types or pastes a token — it's embedded in the page automatically. This means the token is only valid against the local node.

## The JavaScript Client

The gateway includes a dependency-free JavaScript client (`client.js`):

```javascript
// The constructor takes the full WebSocket URL including the token.
const canopee = new CanopeeWeb("ws://127.0.0.1:PORT/?token=…");

// Open the connection (resolves once the gateway replies).
await canopee.connect();

// Store an object — put() resolves to the bare id string.
const id = await canopee.put("Hello, world!");

// Retrieve an object
const obj = await canopee.get(id);

// Subscribe to a topic — subscribe() is synchronous and takes the callback.
const sub = canopee.subscribe("chat-room", (msg) => {
    // msg has { topic, source, text, dataB64 }
    console.log(`${msg.source}: ${msg.text}`);
});

// Publish a message
await canopee.publish("chat-room", "Hello, everyone!");
```

The client implements:

- FIFO response matching (responses are matched to the oldest pending request; requests themselves are sent immediately and may be pipelined)
- Frame dispatch (`ok`/`error`/`pubSub`)
- Methods for every gateway operation

There is no automatic reconnection: when the socket closes, all pending promises are rejected and the client must be re-created.

## WebSocket Implementation

The gateway includes a hand-rolled RFC 6455 WebSocket implementation:

- 30-second handshake timeout
- Rejects fragmented frames (simplified implementation)
- 16 MiB maximum message size
- Token verification on every connection
- Origin enforcement (loopback only)

The implementation is intentionally minimal — it handles the subset of WebSocket needed for the gateway without pulling in a full WebSocket library.

## Security Model

### What the Gateway Protects Against

- **External access**: loopback-only binding means no external page can connect
- **Cross-origin requests**: Origin header verification rejects non-loopback origins
- **CSRF**: session token required on every connection
- **DoS**: Shutdown command is not exposed

### What the Gateway Does Not Protect Against

- **Local malware**: any process on the same machine can connect to the loopback WebSocket
- **Token leakage**: if the HTML page is cached or intercepted, the token is compromised (mitigated by loopback-only)
- **Content confidentiality**: objects are transmitted in plaintext over the WebSocket

## Using the Gateway

### From a Web Application

`client.js` is an IIFE that assigns `global.CanopeeWeb` (no module export). The gateway serves only `GET /` — the demo page with `client.js` inlined; every other non-WebSocket path gets 403. To use it in your own page, inline `client.js` and point it at the gateway URL:

```html
<script>
/* … inlined client.js … */
</script>
<script>
const canopee = new CanopeeWeb("ws://127.0.0.1:PORT/?token=…");
canopee.connect().then(() => {
    canopee.list().then(objects => {
        console.log("Objects:", objects);
    });
});
</script>
```

### From a Framework (React, Vue, etc.)

Because `client.js` has no module exports, load it as a plain script (or inline it) rather than `import`ing it. The API is promise-based:

```javascript
const canopee = new CanopeeWeb("ws://127.0.0.1:PORT/?token=…");
await canopee.connect();

function App() {
    const [peers, setPeers] = useState([]);

    useEffect(() => {
        canopee.peers().then(setPeers);
    }, []);

    return <PeerList peers={peers} />;
}
```

## Testing the Gateway

The gateway includes integration tests with a hand-rolled WebSocket test client:

```bash
cargo test -p canopee-gateway
```

There is a single integration test (kept single because `$HOME` is process-global), plus unit tests for the protocol types. Coverage includes:

- WebSocket handshake and token verification
- Command relay (put, get, list)
- Pub/sub subscription setup (the connection stays responsive; no message is delivered through the gateway in the test node, which has no peers)
- Origin rejection for non-loopback connections
