# Chapter 10: The CLI

## Command-Line Interface

The CLI is the primary tool for interacting with Canopee from the command line. It provides commands for identity management, object storage, networking, app hosting, and more.

```bash
canopee <command> [args...]
```

## Commands

### Identity and Node Management

#### init

Initialize a Canopee identity and storage directory. Does not start a node — just provisions the identity key and creates the directory structure.

```bash
canopee init
```

This is the first command you run. It creates `~/.canopee/` with all necessary subdirectories and generates an Ed25519 keypair.

#### start

Start the Canopee node daemon in the background.

```bash
canopee start
```

The node binds a Unix socket and begins accepting commands. It runs as a detached process.

#### stop

Stop the running node.

```bash
canopee stop
```

#### status

Show node status (running/stopped, identity, object count, peer count). There is no uptime metric; the "running" field is reported by the live node.

```bash
canopee status
```

#### identity

Display the node's identity (PeerId and URI).

```bash
canopee identity
```

### Object Operations

#### put

Store a file as an object.

```bash
canopee put ./myfile.txt
```

Returns the object ID. The file's contents become the object's data payload.

#### get

Retrieve an object by ID. Prints the object's ID (Debug-formatted) — not its contents.

```bash
canopee get <object-id>
```

#### list

List all objects in the store.

```bash
canopee list
```

Shows object IDs, owners, sizes, and signature-validity flags. (`ObjectInfo` has no content-type field.)

#### export

Export an object as a portable bundle. The bincode `ExportBundle` is written by the node to `{app_root}/exports/<object-id>.canopee`; the CLI only prints `Exported: <id>` to stdout (redirecting stdout does not capture the bundle).

```bash
canopee export <object-id>
# Bundle written to {app_root}/exports/<object-id>.canopee
```

#### import

Import a bundle from a file path (stdin is not supported).

```bash
canopee import myobject.canopee
```

### Networking

#### peers

List connected peers with their usernames and display names.

```bash
canopee peers
```

#### dial

Connect to a peer by multiaddress.

```bash
canopee dial /ip4/192.168.1.100/tcp/4001/p2p/12D3KooWABCD...
```

#### listen-via-relay

Connect through a relay node.

```bash
canopee listen-via-relay /ip4/relay.example.com/tcp/4001/p2p/12D3KooWRELAY...
```

#### announce

Announce an object to the DHT.

```bash
canopee announce <object-id>
```

#### fetch

Fetch an object from a specific peer or by username.

```bash
canopee fetch <peer-id-or-username> <object-id>
```

### Usernames

#### claim

Claim a username.

```bash
canopee username claim alice
```

#### show

Show your current username.

```bash
canopee username show
```

#### lookup

Look up a username to find the owner's identity.

```bash
canopee username lookup alice
```

### App Hosting

#### app-manifest

Create and publish an app manifest from a directory of static files.

```bash
canopee app-manifest ./build my-app
```

Walks the directory, creates `Blob` objects for each file, identifies `index.html` as the entrypoint, and publishes an `AppManifest`.

#### app-info

Show information about a published app manifest.

```bash
canopee app-info <manifest-id>
```

#### open

Fetch, resolve, and serve a web application locally.

```bash
# By manifest ID
canopee open <manifest-id>

# By manifest ID from a specific peer
canopee open <manifest-id> --peer <peer-id>

# By owner + name (resolves via DHT)
canopee open --owner <identity-id> --name my-app

# With custom port
canopee open <manifest-id> --port 8080
```

The `open` command:
1. Resolves the manifest (locally, from a peer, or via DHT)
2. Fetches all referenced assets
3. Starts a local HTTP server on `127.0.0.1`
4. Serves the application with proper content types, ETags, and keep-alive

### Chat

#### chat

Interactive pub/sub chat on a topic.

```bash
canopee chat my-room
```

Opens a REPL where:
- Type a message and press Enter to publish
- Incoming messages are printed in the background
- Press Ctrl-D (stdin EOF) to exit — there is no `/quit` command

### URI Parsing

The CLI understands Canopee URIs:

```
canopee://identity/<peer-id>
canopee://identity/<peer-id>/<record-name>
canopee://<alias>/<record-name>
```

URI resolution is handled by the dedicated `canopee handle <uri>` command (and the `alias` commands manage the local alias table):

```bash
canopee handle canopee://alice/my-app
canopee alias set alice canopee://identity/12D3KooWABCD...
canopee alias list
```

Note: `canopee open` does *not* parse URIs — its positional argument is a raw manifest ObjectId. Use `canopee handle` to resolve a `canopee://` URI to an app.

## The App Hosting Server

The `open` command's built-in HTTP server is a hand-rolled implementation with:

- **Keep-alive**: persistent connections for performance
- **Range requests**: partial content delivery
- **Gzip compression**: for text-based assets
- **ETag/304**: conditional requests to avoid re-sending unchanged assets
- **HEAD support**: for checking asset metadata without downloading
- **Loopback-only**: binds to `127.0.0.1` — never exposed to the network

The server maps requested paths to asset ObjectIds from the manifest. All assets are fetched up front (from the local store, the publishing peer, or DHT providers) before the server starts; the HTTP server itself serves only from an in-memory map and never fetches at request time.

## Publishing a Directory

When you run `canopee app-manifest ./build my-app`, the CLI:

1. Walks the directory recursively
2. Identifies `index.html` as the entrypoint
3. Creates a `Blob` object for each file
4. Publishes each blob to the DHT
5. Constructs an `AppManifest` mapping paths to ObjectIds
6. Publishes the manifest as an `AppPointer` record

The resulting application can be served by any peer that has the manifest — including peers that don't have the original files. The distributed cache handles asset delivery.

## Other Commands

The CLI has additional commands not covered above:

- `relay-status` — list active relay circuit reservations
- `find-providers <object-id>` — query DHT providers for an object
- `share <name> <object-id> [--app <app>]` / `unshare <name>` — toggle sharing
- `home` — show the local HomeIndex
- `publish <name> <object-id>` — publish a `(owner, name)` record
- `handle <uri>` — resolve and open a `canopee://` URI
- `alias set <name> <identity-uri>` / `alias list` / `alias remove <name>` — manage local aliases
- `uri-register` / `uri-unregister` — register the `canopee://` scheme with the OS
- `gateway [--port <port>]` — start the browser WebSocket gateway
- `open <id> --open` — also opens the served app in the browser

## Error Messages

Much of the CLI's output is raw `Debug` formatting for protocol responses. This is intentional — Canopee is a developer tool, and the output is designed for direct inspection and debugging. Expect structured but not prettified output (though several commands, such as `peers` and `status`, do print labeled human-readable lines).
