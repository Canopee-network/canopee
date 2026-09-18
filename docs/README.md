# Canopee documentation

The documentation is organized into three layers, following the
concepts → guides → reference structure:

## Concepts — *how it works*

Understand the model before you touch a command.

- [Architecture](concepts/architecture.md) — nodes, identities, objects, and
  the crate map.
- [Identity](concepts/identity.md) — one identity per person, many devices,
  pairing, usernames, sync.
- [Objects & pointers](concepts/objects.md) — the content-addressed object
  model, pointers, records, the home index.
- [Networking](concepts/networking.md) — mDNS, the DHT, relays, hole
  punching, pub/sub.
- [Sharing](concepts/sharing.md) — nothing is shared by default; how objects
  move between peers.
- [Capabilities](concepts/capabilities.md) — signed, verifiable grants over
  resources.
- [Security](concepts/security.md) — what's handled, what's not, and where
  your app's responsibilities start.

## Guides — *how to do it*

Hands-on, command-by-command walkthroughs.

- [Quickstart](guides/quickstart.md) — get a node up and objects stored in
  five minutes.
- [Sharing between peers](guides/sharing.md) — share → discover → fetch →
  unshare.
- [Chatting between peers](guides/chat-between-peers.md) — live pub/sub.
- [Multi-device identity](guides/multi-device-identity.md) — one identity on
  many devices.
- [Usernames](guides/usernames.md) — be addressable by a friendly name.
- [Publishing static apps](guides/publishing-apps.md) — app manifests,
  pointers, open.
- [`canopee://` URIs](guides/uri-scheme.md) — links into the network from the
  OS and browser.
- [Browser gateway](guides/gateway.md) — drive the node from a browser tab.
- [Content caching](guides/content-caching.md) — how fetched content
  replicates.
- [Capabilities in practice](guides/capabilities.md) — issue, verify, revoke.
- [Bootstrap & relay nodes](guides/bootstrap-nodes.md) — run your own relay
  or private network.
- [Running the test suite](guides/testing.md) — unit → integration → e2e.
- [Build a Tauri app](guides/tauri-quickstart.md) — embed the runtime, no
  separate node process.
  - [Tauri chat app](guides/tauri-chat-app.md)
  - [Tauri collab editor](guides/tauri-collab-editor.md)
  - [Tauri multiplayer game](guides/tauri-multiplayer-game.md)
  - [Tauri presence app](guides/tauri-presence-app.md)

## Reference — *the exacts*

Exhaustive, machine-checkable detail.

- [CLI reference](reference/cli.md) — every `canopee` command.
- [Environment variables](reference/environment.md) — every `CANOPEE_*`.
- [Filesystem layout](reference/filesystem.md) — the `~/.canopee` map.
- [Capabilities reference](reference/capabilities.md) — the `canopee cap`
  surface.
- [Protocols & DHT records](reference/protocols.md) — wire shapes and record
  keys.
- [SDK client reference](reference/sdk.md) — the `CanopeeClient` API.
- [Deployment](reference/deployment.md) — the node as a systemd service and
  public relay.
- [Roadmap](reference/roadmap.md) — published plans and app ideas.
- [Comparison](reference/comparison.md) — Canopee vs. IPFS, Iroh, Nostr.

---

**Historical content.** Everything archived before the 2026 docs rewrite
lives under [`archive/`](archive/) — including the old `canopee-book`
chapters and the pre-rewrite per-topic docs. Deleted-to-archive, not deleted:
the current guides supersede them.

**Operational resources** live next to the code, not in this tree:
[`deploy/README.md`](../deploy/README.md) (systemd relay install) and
[`scripts/e2e.sh`](../scripts/e2e.sh) (the two-node e2e shell test).