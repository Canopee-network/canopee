# Quickstart

Get a node running and your first objects stored in about five minutes.

## 1. Build

```bash
cargo build --release -p canopee-node -p canopee-cli
```

or just `cargo build --release` for everything. The release binaries are
`target/release/canopee-node` and `target/release/canopee-cli` (also aliased
as `canopee`).

## 2. Initialize

```bash
canopee init
```

This creates `~/.canopee`: an identity key `identity/identity.key`, a
per-device key `identity/device.key`, and the storage/records/state
directories. See the [filesystem layout](../reference/filesystem.md) for
what's what. `init` is safe to re-run.

## 3. Start the node

```bash
canopee start
```

The daemon runs in the background and speaks `canopee-protocol` over
`~/.canopee/node.sock`. Not running a separate binary for embedded apps —
see [architecture](../concepts/architecture.md) — but for the CLI this is
the entry point.

```bash
canopee status    # is it up? identity? peer count?
canopee identity  # this node's identity: canopee://identity/<peer-id>
canopee device    # this machine's network PeerId + device name
```

## 4. Store objects

```bash
echo "hello p2p" > hello.txt
canopee put hello.txt        # store a file, signed by your identity
canopee list                 # what's stored locally
canopee get hello.txt        # retrieve it by the name you stored it under
canopee desc hello.txt       # metadata (owner, type, size) + preview
```

> Objects are **private by default**: `put` stores locally, it does not
> announce anything to the network. Sharing is a separate, explicit step —
> see the [Sharing guide](sharing.md).

## 5. Talk to other nodes

On a shared LAN you may already be connected (mDNS). Check:

```bash
canopee peers
```

If you know a peer's address you can dial it directly:

```bash
canopee dial /ip4/203.0.113.7/tcp/4001/p2p/12D3KooW...
```

Over the internet, nodes meet through the DHT and the default bootstrap
relay ([networking concept](../concepts/networking.md)). A quick way to see
the whole sharing loop is the two-node walkthrough in
[Sharing between peers](sharing.md).

## 6. What you might want next

- [Sharing between peers](sharing.md) — make an object fetchable, by name
  and by id.
- [Chatting between peers](chat-between-peers.md) — how the network moves
  live messages, end to end.
- [Multi-device identity](multi-device-identity.md) — bring this identity to
  another machine (pair / sync / export-import).
- [Publishing static apps](publishing-apps.md) — turn a directory into a
  fetchable, openable app.
- [Usernames](usernames.md) — be addressable by a friendly name.
- Full command reference: [CLI reference](../reference/cli.md).

## Troubleshooting

| Symptom | Likely cause / fix |
|---|---|
| `canopee status` says no node | `canopee start` first; check `$HOME/.canopee/node.sock` exists |
| Two instances on one machine collide | Use `CANOPEE_APP_ROOT=/tmp/other canopee …` to isolate (see [environment](../reference/environment.md)) |
| `peers` is empty on a LAN | mDNS may be off (`CANOPEE_MDNS`) or blocked; try `canopee dial` with a known address |
| Can't reach peers over the internet | Ensure `?bootstrap` works: your node uses the default relay; `canopee relay-status` shows reservations |