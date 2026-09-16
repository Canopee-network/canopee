# Chapter 12: Application Distribution

## The App Manifest

Canopee provides a built-in system for distributing and serving web applications. The `AppManifest` describes a published application:

```rust
struct AppManifest {
    name: String,
    owner: IdentityId,
    entrypoint: ObjectId,              // index.html
    assets: HashMap<String, ObjectId>, // path → ObjectId mapping
}
```

An app manifest is a pointer document — it contains no file bytes. Each asset is its own `Blob` object in the store. This means:

- Assets can be served by any peer that has them
- The manifest is small and cheap to publish

Note: assets are content-addressed but *not* deduplicated across versions — the payload includes a fresh `created_at` timestamp on every put, so republishing identical file bytes produces a different ObjectId each time.

## Publishing an Application

### From a Directory

```bash
canopee app-manifest ./build my-app
```

The CLI walks the build directory:

1. **Find the entrypoint**: looks for `index.html` at the root
2. **Create Blob objects**: each file becomes a `Blob` object with appropriate metadata
3. **Publish assets**: each blob is announced to the DHT
4. **Construct the manifest**: maps file paths to ObjectIds
5. **Publish the manifest**: creates an `AppPointer` record pointing to the manifest

### The Publish Walk

The directory walk is thorough:

```
./build/
├── index.html          → Blob object → entrypoint
├── style.css           → Blob object → assets["/style.css"]
├── app.js              → Blob object → assets["/app.js"]
├── logo.png            → Blob object → assets["/logo.png"]
└── fonts/
    └── inter.woff2     → Blob object → assets["/fonts/inter.woff2"]
```

Each asset's path is preserved in the manifest (with a leading `/`), so the serving layer can map HTTP requests to ObjectIds.

### Validation

The publish process performs two checks, both after the Blob objects have been created:

- The entrypoint (`index.html`) exists
- Any absolute `src="/..."` / `href="/..."` references in `index.html` resolve to files present in the directory

There is no file-size limit and no circular-reference check.

## Resolving an Application

When someone wants to use your app, they need the manifest:

### By Manifest ID (Direct)

```bash
canopee open <manifest-id>
```

If the manifest is in the local store, it's used directly.

### By Owner + Name (DHT Resolution)

```bash
canopee open --owner <identity-id> --name my-app
```

The DHT key `sha256("canopee-app-pointer:" + owner + ":" + name)` is derived, and the record is fetched from the DHT. The record contains the manifest's ObjectId.

### From a Specific Peer

```bash
canopee open <manifest-id> --peer <peer-id>
```

Fetches the manifest from a specific peer.

## Serving an Application

Once resolved, the `open` command serves the application locally:

1. **Fetch the manifest**: get the `AppManifest` object
2. **Fetch assets**: retrieve all assets referenced in the manifest (concurrently)
3. **Start HTTP server**: bind to `127.0.0.1:<port>` (default: random available port)
4. **Serve requests**: map HTTP paths to assets, serve with proper headers

### The HTTP Server

The server is a hand-rolled implementation with production-quality features:

- **Keep-alive**: persistent connections for efficiency
- **Range requests**: partial content delivery for large files
- **Gzip compression**: automatic for text-based content types
- **ETag/304**: conditional requests to avoid re-sending unchanged assets
- **HEAD support**: check asset metadata without downloading
- **Content-Type detection**: proper MIME types for all file types
- **Loopback-only**: binds to `127.0.0.1` — never exposed to the network

### Asset Resolution

When the browser requests `/style.css`:

1. The server looks up `/style.css` in the manifest's `assets` map
2. Gets the corresponding ObjectId
3. Serves the object's data bytes with `Content-Type: text/css`

All assets are fetched (from the local store, the publishing peer, or DHT providers) *before* the server starts; the HTTP server itself serves only from an in-memory map and never fetches at request time.

## Updating an Application

To publish a new version:

```bash
canopee app-manifest ./build-v2 my-app
```

This creates a new manifest with a new ObjectId, and updates the `AppPointer` record to point to the new manifest. Existing peers serving the old version continue to work — the old manifest and assets are still in the store.

Clients that resolve by `(owner, "my-app")` automatically get the latest version.

## Distributed Caching

The distributed cache is built into the object exchange protocol:

1. **Peer A** publishes an app (manifest + assets announced to DHT)
2. **Peer B** fetches the app (resolves manifest, fetches assets)
3. **Peer C** requests the same assets → served by Peer B

By fetching an object, Peer B becomes a provider for it. The more peers that use an app, the more providers exist, and the faster it can be served.

This creates a natural CDN effect without any infrastructure. Popular apps are served from many peers simultaneously.

## SPA Considerations

### Client-Side Routing

Single-page applications with client-side routing (React Router, Vue Router) work out of the box. The server has an SPA fallback: a request for a route-shaped path (no file extension in the last segment) serves the entrypoint `index.html`, while asset-shaped misses (e.g. `/missing.png`) return 404. History-based routing is supported; no hash-routing workaround is needed.

### Asset Paths

The publish-time validator expects assets to be referenced from the repository root. Ensure:

- Vite: set `base: "/"` (the default) in `vite.config.js`
- CRA: leave `homepage` unset in `package.json`

A non-root base (e.g. `base: "./"`) is rejected by the publisher.

### External Resources

Apps that load resources from CDNs (fonts, icons, analytics) require internet access. Canopee serves the app from the local node, but external resources are loaded normally by the browser.

## The App Lifecycle

```
Developer → canopee app-manifest → Publish to DHT
User → canopee open → Resolve manifest → Fetch assets → Serve locally
Browser → http://127.0.0.1:PORT/ → Load index.html → Fetch assets
Other Peers → Object exchange → Cache assets → Serve to new requesters
```

This lifecycle has no servers, no deployment pipelines, no CI/CD. The developer builds locally, publishes to the DHT, and the app is immediately available to any peer that resolves it.
