# App manifests: publishing, announcing, fetching, and opening apps

An **app manifest** is how Canopee represents a small static app (think: a
portfolio site, a single-page app, a set of HTML/CSS/JS/image files) as one
object other peers can discover, fetch, and open in a browser — without a
central server. This doc covers the whole lifecycle: publish → announce →
resolve → fetch → serve, including how an app stays reachable under a
stable name across republishes.

If you're not familiar with objects, identities, or the DHT yet, read
[`networking-for-beginners.md`](networking-for-beginners.md) first.

This is one of two distinct things Canopee calls "apps" — a manifest is
static content Canopee hosts, not a program that talks back to the
network. See [`publishing-vs-building-apps.md`](publishing-vs-building-apps.md)
if you're trying to decide between this and building directly on
[`canopee-sdk`](../crates/canopee-sdk/README.md).

## What's actually in a manifest

`AppManifest` ([`canopee-storage/src/object.rs`](../crates/canopee-storage/src/object.rs))
is a small struct, stored like any other object:

```rust
pub struct AppManifest {
    pub name: String,
    pub owner: IdentityId,
    pub entrypoint: ObjectId,          // the app's index.html, as its own object
    pub assets: HashMap<String, ObjectId>, // "/style.css" -> ObjectId, etc.
}
```

The manifest itself doesn't contain any file bytes — it's a small pointer
document. Every file in the app (`index.html` included) is stored as its own
content-addressed [`Object`](../crates/canopee-storage/README.md), and the
manifest just maps URL paths to those object ids. This is why fetching an app
is a multi-step process: first fetch the manifest, then fetch everything it
points to.

The manifest is itself stored as an `Object` with
`ObjectType::AppManifest` (as opposed to `ObjectType::Blob` for plain files),
so a peer that fetches it can tell what kind of thing it is before decoding.

Because `ObjectId`s are content-addressed, there's no such thing as updating
a manifest in place — republishing new content always produces a brand-new
manifest id. `AppPointerRecord` (below) is what makes a manifest's id feel
stable across republishes, so you don't have to hand out a new id every
time.

## Publishing

```bash
canopee app-manifest ./portfolio alice-portfolio
```

This walks `./portfolio` recursively
([`publish_directory`](../crates/canopee-cli/src/app.rs)):

- Directories are skipped; dotfiles and dot-directories (`.git`, `.DS_Store`,
  ...) are pruned from the walk entirely — only real content gets published.
- Every remaining file is read and stored as a `Blob` object
  (`client.put_file`), printing `<relative path> -> <object id>` as it goes.
- The file named `index.html` at the top level becomes the manifest's
  `entrypoint`. Every other file is added to `assets`, keyed by its path
  with a leading `/` (e.g. `style.css` → `/style.css`).
- If no `index.html` is found, publishing fails — an app manifest needs an
  entrypoint to be openable.

Once every file is stored, the CLI builds the `AppManifest`, stores it as an
`ObjectType::AppManifest` object, **announces** the manifest and every asset
on the DHT, and **publishes a signed pointer** under the given name (all
three of these are covered in the next two sections):

```
$ canopee app-manifest ./portfolio alice-portfolio
index.html -> 3041bc14...
style.css -> 9a7d21ee...

Application published and announced:
c7e5a02f...

Republishing under the same name ("alice-portfolio") will update what
`open --owner ... --name alice-portfolio` resolves to.
```

The printed id — the manifest's own `ObjectId` — still works as a direct
`open <id>` target, same as before. But you no longer need to keep track of
it or share a new one after every republish; see
[Resolving by name](#resolving-by-name-app-pointers) below.

To inspect a manifest you already have locally (yours or one you've fetched):

```bash
canopee app-info c7e5a02f...
```

This does a plain local `Get` and decodes the object as an `AppManifest`,
printing `name`, `owner`, `entrypoint`, and `assets`.

## Announcing (making it discoverable)

Storing an object locally doesn't make it visible to anyone else. Discovery
on Canopee works through the Kademlia DHT: a node **announces** that it
*provides* a given `ObjectId`, and other nodes can later call
`find_providers` on that same id to get back a list of peers that announced
it.

```bash
canopee announce c7e5a02f...
```

At the SDK level this is `client.announce(id)`, which sends
`NodeCommand::Announce` to the node, which calls `NetworkManager::announce`
— this puts a provider record into the DHT under that object's key
([`canopee-network`](../crates/canopee-network/README.md)).

`canopee app-manifest` calls this automatically for you — the manifest, the
entrypoint, and every asset all get announced as part of publishing, so
there's nothing extra to run in the common case. You'd reach for
`canopee announce` directly if you're re-announcing something (announcements
aren't permanent — they can expire from the DHT and may need repeating) or
announcing an object that wasn't published through `app-manifest`.

If you skip announcing entirely (or the DHT hasn't propagated your
announcement to the fetcher's neighborhood yet), that's fine — `open`'s
`--peer` flag lets a fetcher route around discovery entirely by asking a
specific peer directly.

## Resolving by name (app pointers)

Sharing a manifest id works, but it changes every time you republish. An
**app pointer** is a signed, mutable record that maps a stable `(owner,
name)` pair to the *current* manifest id — a name Alice picks once (e.g.
`"alice-portfolio"`) keeps resolving to whatever she most recently
published under that name, without her needing to tell anyone a new id.

`canopee app-manifest` publishes one of these automatically (that's the
`publish_app_pointer` call visible in the "Republishing..." message above).
Under the hood:

1. The **node** (not the CLI or SDK client — only the node holds the
   private key) signs an `AppPointerRecord { name, owner, manifest,
   published_at, ... }` via `AppPointerRecord::sign`
   ([`canopee-storage`](../crates/canopee-storage/README.md)).
2. It's published as a DHT record — `NetworkManager::put_record` — under a
   key derived purely from `owner` + `name`
   (`AppPointerRecord::key`), *not* the record's own content. That's what
   makes it mutable: republishing under the same name calls `put_record`
   again with the same key, overwriting the previous value network-wide.
3. A fetcher who only knows `(owner, name)` derives the same key
   independently and calls `NetworkManager::get_record` to look it up — no
   need to exchange the key out of band, just the owner's identity string
   and the name.

Resolution happens via `canopee open --owner <id> --name <name>` — see
below. A resolved pointer is only trusted after `record.verify()` passes:
this checks the signature against the embedded public key *and* that the
public key actually corresponds to the claimed `owner`, so a malicious peer
can't serve you a pointer claiming someone else's identity.

## Fetching over the network

This is handled by
[`fetch_app`](../crates/canopee-cli/src/app.rs), which resolves a manifest id
into `(AppManifest, HashMap<path, bytes>)`, fetching whatever it doesn't
already have locally. It calls a helper, `get_or_fetch`, for the manifest
and the entrypoint, then fetches every asset **concurrently**:

1. **Try local storage first** (`client.get(id)`) — if you already have the
   object (you're the owner, or you fetched it before), no network call
   happens at all.
2. **Resolve a peer, if one isn't already known** — if you passed `--peer`,
   that's used directly. Otherwise it calls `find_providers(id)` and takes
   the first peer id returned. This happens once, right after the manifest
   is resolved, and the same peer is reused for the entrypoint and every
   asset — in practice they're all provided by the same node (the app's
   owner), so there's no reason to re-run discovery per object.
3. **Fetch and import** — `client.fetch_object(peer_id, id)` sends a
   `NodeCommand::FetchObject`, which the node turns into a libp2p
   request/response exchange on the `/canopee/objects/1.0.0` protocol
   ([`canopee-network`](../crates/canopee-network/README.md)) and returns an
   `ExportBundle`. The fetched object is then `import`ed into local storage,
   so it doesn't need to be re-fetched next time (and so it can itself be
   served up to a third peer, if you re-announce it).

Because the resolved peer is shared, fetching the entrypoint plus every
asset for step 3 all happen at once via `futures::future::try_join_all`
rather than one request at a time — a portfolio with a dozen images fetches
in roughly the time of the single slowest request, not the sum of all of
them.

If no `--peer` is given and `find_providers` comes back empty (nobody
announced the object, or the DHT hasn't propagated the record yet), fetching
fails with `no providers found for <id>; try passing --peer`.

## Serving / opening an app

Once every object is resolved, the app is served over plain local HTTP
([`serve`](../crates/canopee-cli/src/app.rs)) — no browser extension or
special protocol handler needed. Two ways to point `open` at an app:

```bash
# Directly by manifest id:
canopee open c7e5a02f... [--peer <peer-id>] [--port <port>]

# Or by the publisher's stable (owner, name) pointer — always resolves to
# whatever they most recently published under that name:
canopee open --owner "canopee://identity/12D3KooW...alice" --name alice-portfolio [--peer <peer-id>] [--port <port>]
```

- `--owner`/`--name` — resolves the manifest id via `resolve_app_pointer`
  before doing anything else. Requires both flags together; mutually
  exclusive with passing a manifest id directly.
- `--peer` — the fetcher's target peer id (see [Announcing](#announcing-making-it-discoverable));
  skip this if the publisher announced the manifest/assets and the DHT has
  already picked that up. When resolving by `--owner`/`--name`, `--peer` is
  also used (if given) to look up the pointer record itself, not just the
  manifest/assets afterward.
- `--port` — defaults to `0`, meaning "let the OS pick a free port." The
  actual bound address is printed (`Serving app at http://127.0.0.1:PORT`) —
  open that URL in a browser.

Internally this is a small hand-rolled HTTP/1.1 server over
`tokio::net::TcpListener` (deliberately not pulling in a web framework for
something this small): each connection is read just far enough to grab the
request line's path, that path is looked up in the in-memory `path -> bytes`
map built by `fetch_app` (the entrypoint is always served at `/`), and either
a `200` with a guessed `Content-Type` (by file extension) or a `404` is
written back. The server runs until the process is killed — there's no
"serve once and exit" mode.

## End-to-end: Alice publishes, Pierre opens it, Alice republishes

**Alice:**

```bash
canopee start
canopee app-manifest ./portfolio alice-portfolio
# Application published and announced:
# c7e5a02f...

canopee identity
# IdentityId("canopee://identity/12D3KooW...alice")
```

**Pierre**, on a node connected to Alice's (directly dialed, via relay, or
mDNS on the same LAN — see
[`testing-chat-between-peers.md`](testing-chat-between-peers.md) for how to
get two nodes talking):

```bash
canopee start
canopee open --owner "canopee://identity/12D3KooW...alice" --name alice-portfolio --peer 12D3KooW...alice
# Opening "alice-portfolio" by canopee://identity/12D3KooW...alice
# Serving app at http://127.0.0.1:54291
```

Pierre opens `http://127.0.0.1:54291` in a browser and sees Alice's
portfolio, fetched entirely peer-to-peer. Note Pierre never learned a
manifest id — only Alice's identity string and the name she chose.

**Alice updates her site and republishes** under the same name:

```bash
canopee app-manifest ./portfolio alice-portfolio
# Application published and announced:
# 91af2c30...   <- a brand-new manifest id
```

**Pierre re-runs the exact same `open` command** (same `--owner`/`--name`,
no id to update):

```bash
canopee open --owner "canopee://identity/12D3KooW...alice" --name alice-portfolio --peer 12D3KooW...alice
# Opening "alice-portfolio" by canopee://identity/12D3KooW...alice
# Serving app at http://127.0.0.1:54292
```

This now serves Alice's *updated* content — the pointer under
`alice-portfolio` was overwritten by the republish, so resolving it again
picks up `91af2c30...` instead of the old `c7e5a02f...`, with no new id
shared out of band.

## Known limitations

- Only the object explicitly requested is discovered via `find_providers`;
  there's no "announce/fetch this whole app's object graph" convenience
  beyond what `app-manifest` already does automatically at publish time —
  if you build a manifest by some other path, you're responsible for
  announcing everything it references yourself.
- `AppPointerRecord`s carry a `published_at` timestamp but nothing enforces
  monotonicity on write — `put_record` unconditionally overwrites whatever
  was at the key. A resolver that already has a newer pointer cached
  doesn't currently check `published_at` before accepting an older one
  served back to it by a stale DHT replica.
- DHT records (including app pointers) aren't durable forever — Kademlia
  records expire and rely on republishing to stay alive. A pointer or
  provider announcement that hasn't been refreshed in a while may need to
  be re-published/re-announced.
- An app is only reachable while its original publisher's node is online —
  `open` only ever fetches from whoever `find_providers` returns, and today
  that's only the publisher, since fetchers don't announce what they cache.
  See [`p2p-app-caching-tutorial.md`](p2p-app-caching-tutorial.md) for a
  guided walkthrough of building fetch-and-reannounce caching so apps
  survive their publisher going offline.
- `serve` does an exact path match against the published `assets` map, with
  no fallback for unknown paths beyond a `404`. This is fine for a
  hand-written static page, but a single-page app (React Router, etc.)
  needs unknown routes to fall back to `index.html` for direct links and
  refreshes to work — and `guess_content_type` doesn't yet cover fonts,
  icons, or other common bundler output. See
  [`spa-hosting-tutorial.md`](spa-hosting-tutorial.md) for a guided
  walkthrough of publishing a real React/Vite build correctly.
