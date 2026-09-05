# canopee-sdk

The client library apps use to talk to a running [Canopee
node](../canopee-node): identity, object storage, and peer-to-peer network
operations, all over the node's Unix socket. This is the crate an app
depends on — it never needs [`canopee-network`](../canopee-network) or
[`canopee-storage`](../canopee-storage) directly, and never touches libp2p.

```toml
[dependencies]
canopee-sdk = { path = "../canopee-sdk" } # or a registry/git dependency once published
```

If you're trying to publish a static site/SPA rather than build a program
that talks to the network itself, you don't need this crate at all — see
[`docs/app-manifests.md`](../../docs/app-manifests.md) and
[`docs/publishing-vs-building-apps.md`](../../docs/publishing-vs-building-apps.md)
for the distinction.

## Quick start

```rust
use canopee_sdk::CanopeeClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Connects to the locally running node. Errors if it isn't running —
    // start one with `cargo run -p canopee-node` or `canopee start` first.
    let client = CanopeeClient::connect().await?;

    let id = client.identity().await?;
    println!("connected to {id}");

    let object_id = client.put(b"hello canopee".to_vec()).await?;
    let object = client.get(object_id).await?;
    println!("{}", String::from_utf8_lossy(&object.payload.data));

    Ok(())
}
```

## `CanopeeClient`

The main entry point. Every method makes one round trip to the node over
its Unix socket (see [`canopee-protocol`](../canopee-protocol) for the wire
format) except `subscribe`, which opens a persistent connection.

### Identity

```rust
let id: canopee_sdk::IdentityId = client.identity().await?;
```

### Storage

```rust
let id = client.put(data).await?;                 // store bytes, signed by the node's identity
let object = client.get(id.clone()).await?;         // read back, pre-verified
let objects = client.list().await?;                 // everything in local storage
let bundle = client.export(id.clone()).await?;       // portable, self-verifying bundle
client.import(bundle).await?;                        // bring a bundle into local storage
```

### Network

```rust
let peers = client.peers().await?;                                       // currently connected peers
client.dial("/ip4/1.2.3.4/tcp/4001/p2p/<peer id>").await?;                // connect directly
client.listen_via_relay("/ip4/5.6.7.8/tcp/4001/p2p/<relay id>").await?;   // enable hole punching via a relay

client.announce(id.clone()).await?;                                      // "I have this object" (DHT)
let providers = client.find_providers(id.clone()).await?;                // who else has it
let bundle = client.fetch_object(providers[0].clone(), id).await?;       // fetch it from them directly

client.publish_app_pointer("alice-portfolio", manifest_id).await?;       // sign + publish (owner, name) -> manifest
let latest = client.resolve_app_pointer(owner_id, "alice-portfolio").await?; // None if not found or unverifiable

client.publish("app-topic", b"hello".to_vec()).await?;                   // gossipsub publish
```

### User records ("state is per-app, data is per-user")

```rust
// Generic pointer.
client.publish_pointer("app:demo", object_id).await?;
let record = client.resolve_pointer(owner_id, "app:demo").await?;

// Profile / contacts / home index (reserved record names are re-exported
// as RECORD_PROFILE / RECORD_CONTACTS / RECORD_HOME).
let id = client.save_profile(&profile).await?;
let profile = client.load_profile().await?;          // Option<Profile>
client.save_contact_list(&list).await?;
let list = client.load_contact_list().await?;        // Option<ContactList>
client.save_home_index(&index).await?;
let index = client.load_home_index().await?;         // Option<HomeIndex>
```

Saving bakes a server-side version bump and repoints the reserved record —
clients never sign anything themselves (the node holds the key).

### Sharing gate

```rust
client.share_object("pic.png", id, Some("photos".into())).await?; // upsert a shared entry + announce
client.set_home_entry_shared("pic.png", false).await?;            // flip one entry's flag
client.unshare("pic.png").await?;                                  // shorthand for the above
let index = client.load_home_index().await?;                      // see what's shared
```

`shared: true` entries mean the object is announced on the DHT and served
to any peer; `false` entries are only ever in the owner's store. Nothing is
shared by default.

### Pub/sub subscriptions

`subscribe` returns a [`Subscription`] that streams messages until dropped:

```rust
let mut subscription = client.subscribe("app-topic").await?;
while let Some(message) = subscription.next().await? {
    println!("[{}] {}", message.topic, String::from_utf8_lossy(&message.data));
}
```

Dropping the `Subscription` (or letting it go out of scope) closes the
underlying connection, which the node treats as an unsubscribe — there's no
separate unsubscribe call to remember.

### Shutdown

```rust
client.shutdown().await?; // asks the node to stop
```

## Full end-to-end example

See [`examples/app.rs`](examples/app.rs) for a small CLI-style app exercising
every capability:

```bash
cargo run -p canopee-sdk --example app -- identity
cargo run -p canopee-sdk --example app -- put "hello"
cargo run -p canopee-sdk --example app -- get <object-id>
cargo run -p canopee-sdk --example app -- peers
cargo run -p canopee-sdk --example app -- announce <object-id>
cargo run -p canopee-sdk --example app -- find-providers <object-id>
cargo run -p canopee-sdk --example app -- fetch <peer-id> <object-id>
cargo run -p canopee-sdk --example app -- publish <topic> "hello"
cargo run -p canopee-sdk --example app -- subscribe <topic>
```

## `NodeClient`

`CanopeeClient` is built on `NodeClient`, the low-level request/response
transport (connect, frame, serialize/deserialize). It's exported for callers
that need to send a raw `canopee_protocol::NodeCommand` directly — e.g. the
[CLI](../canopee-cli), which predates `CanopeeClient` and still talks to the
protocol layer itself for a couple of commands. New apps should prefer
`CanopeeClient`.

## Design notes

- `CanopeeClient::connect()` checks `is_running()` up front and fails fast
  with a clear error rather than letting the first real request time out
  confusingly if no node is listening.
- Every fallible operation maps a `NodeResponse::Error { message }` (or any
  unexpected response variant) into an `anyhow::Error` via one shared
  `unexpected()` helper, so callers get a consistent `anyhow::Result<T>`
  across the whole API regardless of which command failed.
- `publish_app_pointer` only takes a `name` and a manifest `ObjectId` —
  signing happens inside the node, not the SDK, because only the node holds
  the private key behind its `Identity`. The same rule applies to the
  user-record commands: `save_profile`/`save_contact_list`/`save_home_index`
  send the raw struct, and the node's Runtime signs and repoints reserved
  records. `resolve_app_pointer`/`resolve_pointer`, by contrast, verify
  before returning, so a `None` means either nothing was published or what
  came back failed verification.
- `fetch_object` returns the `ExportBundle` but does **not** import it into
  local storage automatically — callers decide whether/when to persist a
  fetched object via `client.import(bundle)`. This keeps "fetch" and
  "adopt into my storage" as separate, composable steps.
- The streaming nature of `subscribe` is why it's the one method that
  doesn't go through the shared `request()` helper — it needs its own
  connection that stays open, which `Subscription::open` manages directly.

## Testing

```bash
cargo test -p canopee-sdk
```

`tests/client.rs` starts a real `Node` in-process (bound to a scratch
`$HOME`) and drives it purely through `CanopeeClient`, covering identity,
storage, the network methods' error paths, and — over the same socket —
user records (pointer round-trip, profile/contact/home versioning) and the
share/unshare flip. It depends on [`canopee-node`](../canopee-node) as a
dev-dependency only — the SDK itself never depends on the node crate at
runtime.
