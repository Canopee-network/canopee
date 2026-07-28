# canopee-protocol

The wire format between a running [Canopee node](../canopee-node) and its
clients ([`canopee-sdk`](../canopee-sdk), [`canopee-cli`](../canopee-cli)).
Pure data — no I/O, no logic — just the `NodeCommand`/`NodeResponse` enums
and the small value types they carry, all `serde`-derived.

This crate exists so the node and its clients can depend on the same types
without the client needing to depend on the node's runtime, storage
internals, or network stack.

## Framing

Every command and response is `bincode`-serialized and sent as a `u32`
length prefix followed by that many bytes, over a Unix domain socket. Both
[`canopee-node`](../canopee-node) and
[`canopee-sdk::NodeClient`](../canopee-sdk) implement this framing
independently (it's a few lines); `canopee-protocol` only defines what gets
serialized, not how the bytes are moved.

## Commands and responses

Most commands are simple request → response:

| Command | Response | Purpose |
|---|---|---|
| `Identity` | `Identity { identity_id }` | This node's identity |
| `Put { data }` | `ObjectCreated { id }` | Store bytes as a signed object |
| `Get { id }` | `Object { object }` | Read a locally stored object |
| `List` | `Objects { objects }` | List locally stored objects |
| `Export { id }` | `Exported { bundle }` | Export a local object as a portable bundle |
| `Import { bundle }` | `Imported` | Import a bundle into local storage |
| `Status` | `Status { identity, objects, peers }` | Identity + object count + peer count |
| `Shutdown` | `ShutdownAccepted` | Ask the node to stop |
| `Dial { addr }` | `Dialed` | Connect directly to a peer multiaddr |
| `ListenViaRelay { relay_addr }` | `ListeningViaRelay` | Request a relay circuit reservation |
| `Publish { topic, data }` | `Published` | Publish to a gossipsub topic |
| `Peers` | `Peers { peers }` | Currently connected peers |
| `FindProviders { id }` | `Providers { peer_ids }` | Who on the DHT has announced this object |
| `FetchObject { peer_id, id }` | `Exported { bundle }` | Fetch an object directly from a specific peer |
| `Announce { id }` | `Announced` | Announce on the DHT that this node holds an object |
| any command | `Error { message }` | Any of the above can fail with this instead |

`Subscribe { topic }` is the one exception — see below.

### `Subscribe` is a streaming command, not request/response

Every other command gets exactly one response and the connection closes.
`Subscribe` **hijacks the connection**: the node sends one `Subscribed`
frame, then keeps pushing `PubSub(PubSubMessage)` frames on the *same*
connection for as long as the client stays connected. There is no
`Unsubscribe` command — closing the connection (dropping
[`Subscription`](../canopee-sdk)) is how a client unsubscribes.

```rust
// what canopee-sdk's Subscription does, roughly:
send(Subscribe { topic })
recv() -> Subscribed                  // or Error
loop {
    recv() -> PubSub(message)         // repeats until disconnect
}
```

This is a deliberate protocol asymmetry: request/response doesn't fit an
unbounded stream of future messages, and a separate polling command would
add latency and complexity for no benefit over just keeping the socket open.

## Design notes

- `NodeCommand`/`NodeResponse` are flat enums, not a trait or RPC framework
  — adding an operation means adding a variant to each and a match arm in
  [`canopee-node`](../canopee-node)'s `handle`. This keeps the protocol
  fully explicit and easy to read end-to-end in two files.
- `PeerInfo`/`PubSubMessage` here are protocol-level DTOs, distinct from
  [`canopee-network`](../canopee-network)'s `Peer`/`PubSubMessage` types —
  the network crate's types use libp2p types (`PeerId`, `Multiaddr`)
  directly, which aren't (and shouldn't be made to be) part of the wire
  contract; the protocol crate's versions use plain `String`s instead so
  clients never need a libp2p dependency just to talk to the node.

## Testing

No behavior to test — this crate is data definitions only.
