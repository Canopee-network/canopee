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
| `Put { data }` | `ObjectCreated { id }` | Store bytes as a signed `Blob` object |
| `PutObject { data, object_type }` | `ObjectCreated { id }` | Store bytes as a signed object of a specific `ObjectType` (e.g. `AppManifest`) |
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
| `PublishAppPointer { name, manifest }` | `AppPointerPublished` | Sign (with this node's identity) and publish a mutable `(owner, name) -> manifest` pointer to the DHT, overwriting any previous pointer under the same name |
| `ResolveAppPointer { owner, name }` | `AppPointer { record }` | Look up the latest pointer published by `owner` under `name`; `record` is `None` if not found. Caller must call `record.verify()` before trusting `record.manifest` |
| `PublishPointer { name, target }` | `PointerPublished` | Generic user-record publish through the Runtime's cache-aware pointer layer (local record cache + best-effort DHT) |
| `ResolvePointer { owner, name }` | `Pointer { record }` | Resolve a user record the Runtime way (local cache first, then bounded DHT) |
| `SaveProfile { profile }` | `ProfileSaved { id }` | Store a new `Profile` version and repoint `(owner, "profile")` |
| `LoadProfile` | `Profile { profile }` | Load the current `Profile` (local cache) |
| `SaveContactList { list }` | `ContactListSaved { id }` | Store a new `ContactList` snapshot and repoint `(owner, "contacts")` |
| `LoadContactList` | `ContactList { list }` | Load the current `ContactList` |
| `SaveHomeIndex { index }` | `HomeIndexSaved { id }` | Store a new `HomeIndex` version and repoint `(owner, "home")` |
| `LoadHomeIndex` | `HomeIndex { index }` | Load the current `HomeIndex` |
| `SetHomeEntryShared { name, shared }` | `HomeIndexSaved { id }` | Flip a home entry's `shared` flag (the share/unshare action) |
| `ShareObject { name, object, app }` | `HomeIndexSaved { id }` | Upsert a `shared: true` home entry for an object and announce it on the DHT |
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
- `PublishAppPointer` takes a `name` and `manifest` id, not a pre-signed
  `AppPointerRecord` — signing happens inside `canopee-node`'s handler,
  because only the node holds the `Identity`'s private key. A client (SDK or
  CLI) can never construct a validly-signed pointer itself, only ask the
  node to. `ResolveAppPointer`'s response, by contrast, carries the full
  [`AppPointerRecord`](../canopee-storage) (signature included) since the
  caller — not the node — is responsible for verifying it before trusting
  `record.manifest`.
- The same signing-in-the-node rule applies to the **user-record commands**
  (`SaveProfile`/`SaveContactList`/`SaveHomeIndex`/`ShareObject`/
  `PublishPointer`): clients pass the raw `Profile`/`ContactList`/`HomeIndex`
  (or name + object id), and the node's Runtime bakes the version bump,
  encodes it as a signed `Object`, and repoints the reserved record. This
  keeps record authorship unspoofable without the SDK needing signing
  primitives.
- `ShareObject`/`SetHomeEntryShared` are how the **serving gate** is flipped
  over the socket: they return the new `HomeIndex` object id so clients can
  track the current index version in one round trip.

## Testing

No behavior to test — this crate is data definitions only.
