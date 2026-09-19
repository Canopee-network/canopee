# Publishing static apps

A **published app** is static content — HTML/CSS/JS/images — wrapped in a
signed `AppManifest` object, discoverable on the DHT, fetchable by peers, and
served to a browser by `canopee open` over plain HTTP. Nothing is running;
it's exactly what a static host does, with content-addressed, signed
objects instead of a SaaS.

This is one of two things Canopee calls "apps" — see
[vs. SDK apps](#published-app-vs-sdk-app) at the end if you're deciding.

## Publish a directory

```bash
canopee app-manifest ./portfolio alice-portfolio
```

What this walks and does:

1. stores every file as a `Blob` object (dotfiles and `.git` pruned),
2. builds an `AppManifest { name, owner, entrypoint: <index.html-id>,
   assets: {"/style.css": <id>, …} }` and stores it as an
   `AppManifest`-typed object,
3. **announces** the manifest and every asset on the DHT (provider records),
4. publishes a **signed pointer** `(owner, "app:alice-portfolio") → manifest-id`.

The pointer is what makes the app resolvable by name across republishes — a
new republish overwrites the pointer, and everyone who opens by name gets the
latest.

Inspect a manifest you already have:

```bash
canopee app-info <manifest-id>
```

## Announce and fetch manually (if needed)

```bash
canopee announce <manifest-id>     # make it discoverable on the DHT
canopee find-providers <manifest-id>  # who is serving it?
canopee fetch <peer> <manifest-id>    # pull it from a specific peer
```

## Open it

```bash
canopee open <manifest-id>
canopee open --owner <identity-id> --name alice-portfolio
```

`open` fetches the manifest, fetches every asset it points to, starts a
tiny local HTTP server, and opens the browser. The app runs entirely from
the local object store — the files are served byte-for-byte, as fetched
or as locally stored.

## `canopee://` URI handling

A published app can be opened by name from the OS and browser (see
[canopee:// URIs](uri-scheme.md)):

```bash
canopee handle canopee://alice/alice-portfolio
```

## Content caching and reachability

Once published and announced, an app is served by any peer that holds its
objects (the publisher, or anyone who fetched it). Fetched content is
content-verified before it's stored, and stored objects are served to future
requesters within the `cached_bytes` LRU — fetching *is* seeding (see
[Content caching](content-caching.md)). A freshly published app
replicates organically as peers fetch it; announcing makes the *first*
fetch possible by advertising the publisher as a provider.

## Serve it to the public internet through an edge

Publishing makes an app *discoverable*; it does not by itself put it on the
public web. For that, Canopee has **edges**: public-facing nodes that accept
HTTP(S) requests for `https://<app-hash>.<domain>/` and tunnel them over
libp2p to the publisher's node, which serves the bytes.

You publish through an edge with a single foreground command — no username,
no configuration:

```bash
canopee publish ./portfolio
```

By default this publishes through the public edge, which is just the
bootstrap relay running its built-in edge role — every `canopee-node` is an
edge unless started with `CANOPEE_EDGE=0`. To publish through a different
edge (e.g. your own domain's), set `CANOPEE_EDGE_ADDR` to that node's libp2p
multiaddr first.

Output:

```text
Publishing "./portfolio" as portfolio
  Manifest: 9f86d0844c2a2b52f0f6e5d4c3b2a19876543210abcdef0123456789abcd
  ✓ Application published!
  Public: https://9f86d0844c2a2b52f0f6e5d4c3b2a198.canopee.network/

  Serving live from this node. Press Ctrl+C to take it offline.
```

The app is live at that URL for as long as the command runs. **Ctrl+C** sends
a signed deregistration to the edge and takes the app offline.

The URL is a **content hash**, not a name: it is derived from the app
manifest (which embeds your identity and every file's hash). Two
consequences:

* **Nobody can take your address.** The edge validates a registration by
  fetching the manifest from the registering peer and checking its owner is
  the identity that signed the claim. There is no username registry to
  squat on or hijack — the hash *is* the address.
* **Republishing changed content gives a new URL** — the manifest hash
  changes. Republishing byte-identical content reproduces the same URL.

What happens under the hood:

1. Every file is stored as an object and a signed
   `app:<dirname>` → manifest pointer is published (the same thing
   `canopee app-manifest` does, with progress suppressed).
2. The node sends a signed **serve registration** for the manifest id to the
   edge (`/canopee/serve-registry/1.0.0`). The edge fetches the manifest
   from the registering peer, verifies it hashes to the claimed id and that
   its `owner` is the signing identity, and only then pins the subdomain.
3. While the session is up, the node re-registers every 30s (the edge drops
   entries that miss 3 beats). Browser requests to
   `https://<app-hash>.canopee.network/…` are routed by the edge's registry
   to your node and answered over `/canopee/serve/1.0.0`.

The public base domain defaults to `canopee.network`; override with
`CANOPEE_PUBLIC_BASE_DOMAIN`. To run your own edge instead of using someone
else's, see [Deploying an edge](../reference/deployment.md#deploying-an-edge).

## Published app vs. SDK app

| | Published app (`app-manifest`) | SDK app (`canopee-sdk` / `Runtime`) |
|---|---|---|
| What it is | Signed `AppManifest` + static files | A process linking `canopee-sdk` |
| Where it runs | In a browser, fetched via `open` | Wherever you run the binary |
| Network capability | None directly; via a [gateway](gateway.md) it can drive the SDK from the tab | Full: `put`/`get`, peers, DHT, pub/sub |
| Analogy | A file on IPFS / a page on Netlify | A daemon, bot, or CLI tool that uses Canopee as its backend |
| When to use | Distributing a static site or SPA | Building something that needs to talk to the network as part of its own logic |

A published SPA and an SDK app aren't mutually exclusive: you can build a
React frontend (published via `app-manifest`) that's meant to be the UI for
a Canopee-native feature. The browser page drives the network through the
local [gateway](gateway.md), while the actual network-facing logic is the
SPA's own JavaScript over that bridge — see the
[Tauri quickstart](tauri-quickstart.md) for the embedded-desktop equivalent
of this pattern, where no gateway is needed because the app links the
Runtime directly.