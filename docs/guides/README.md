# Guides

Hands-on, command-by-command walkthroughs. Read in roughly this order if
you're new; each relies only on what's above it plus the
[Concepts](../concepts/README.md).

## Getting started

- [Quickstart](quickstart.md) — build, initialize, start the node, and store
  your first objects.
- [Sharing between peers](sharing.md) — nothing is shared by default; here's
  how share/fetch/unshare actually behave.
- [Chatting between peers](chat-between-peers.md) — live pub/sub chat on one
  network (LAN + over the public relay).
- [Multi-device identity](multi-device-identity.md) — one identity on many
  devices: pairing, syncing, and exporting/importing the identity key.
- [Usernames](usernames.md) — claim a globally unique `canopee` username and
  be addressable by name.

## Publishing

- [Publishing static apps](publishing-apps.md) — app manifests, app pointers,
  `open`, and serving an app you built in a browser.
- [`canopee://` URIs](uri-scheme.md) — linking into the network from the OS
  and browser.
- [Browser gateway](gateway.md) — a WebSocket bridge so a browser tab can
  drive the node.
- [Content caching](content-caching.md) — how fetched content replicates and
  how to lean on it.

## Operating

- [Bootstrap & relay nodes](bootstrap-nodes.md) — run your own relay, or a
  private network with your own bootstrap.
- [Capabilities in practice](capabilities.md) — issue, verify, and revoke
  signed grants end to end.
- [Running the test suite](testing.md) — the e2e suite and what it proves.

## Building SDK apps

- [yours first Tauri app](tauri-quickstart.md) — wire the `canopee-sdk` into
  a Tauri app.
- [Tauri chat app](tauri-chat-app.md)
- [Tauri collab editor](tauri-collab-editor.md)
- [Tauri multiplayer game](tauri-multiplayer-game.md)
- [Tauri presence app](tauri-presence-app.md)

Each tutorial points at the specific `CanopeeClient` methods it uses —
cross-reference with the [SDK reference](../reference/sdk.md).