# Filesystem layout

The `~/.canopee` directory (root overridable via `CANOPEE_APP_ROOT`, see
[Environment](environment.md)). Paths come from `canopee-config`'s `Config`;
identities and object stores live under the **user root**, runtime state
under the **app root** — the two halves of the "state is per-app, data is
per-user" split.

```
~/.canopee/
├── identity/
│   ├── identity.key        The person's Ed25519 key (same on every device).
│   │                        Optionally encrypted with CANOPEE_IDENTITY_PASS.
│   └── device.key          This machine's Ed25519 device key (never leaves
│                           the device). Its public key == the network PeerId.
├── storage/
│   └── <object-id>         Content-addressed signed objects the user owns.
├── records/
│                           Local cache of resolved (owner, name) pointers,
│                           so apps on one machine see each other's fresh
│                           pointers without DHT round trips.
├── aliases/
│                           Friendly-name → owner map for canopee:// URIs.
├── state/
│   └── node.state          Node runtime state.
├── cache.cache             LRU sidecar: which locally stored objects were
│                           fetched from peers (+ last-served), so the node
│                           evicts cached content without touching its own.
├── exports/                Objects exported via `canopee export`.
├── node.sock               The Unix socket the daemon speaks
│                           canopee-protocol on (canopee-sdk/cli clients).
└── uri-pending             Scratch file for canopee:// URIs handed to the
                            OS handler while no node was running.
```

## The split in practice

- **User root** (`~/.canopee`): `identity/`, `storage/`, `records/`,
  `aliases/` — shared by every app and device of the same person.
- **App root** (also `~/.canopee` for the CLI; overridden by
  `Config::with_app_root` for embedded apps): `state/`, `cache.cache`,
  `exports/`, `node.sock`, `uri-pending` — per-app/per-process runtime state.

`Config::with_root(dir)` collapses both to `dir` (fully isolated — used by
tests and embedded apps that want their own identity). `Config::with_app_root`
moves only the app-root half, keeping the person's identity + data shared.

## New files to expect as you use each feature

| Feature | Files |
|---|---|
| `canopee init` | `identity/identity.key`, `identity/device.key`, `storage/`, `records/`, `state/` |
| any `put/get/share` | objects under `storage/<id>`; home index is a signed object |
| any `fetch` of a peer's object | object under `storage/` + an LRU cache entry |
| `canopee alias set` | `aliases/` |
| `canopee start` | `node.sock` (removed on clean stop) |
| `canopee gateway` | (no persistent files; loopback port + session token) |