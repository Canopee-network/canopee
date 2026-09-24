# Environment variables

All configuration knobs Canopee reads from the environment. Everything is
optional — defaults are chosen so the system works out of the box.

## `CANOPEE_APP_ROOT`

Instance root. Defaults to `$HOME`, so the full layout lands at
`$HOME/.canopee`. Setting it to a different directory relocates
**everything** (identity, storage, network state, socket) under
`<APP_ROOT>/.canopee` — this is how test/CI instances are isolated
(`Config::new()` honors it).

## `CANOPEE_MDNS`

Multi-cast DNS (LAN discovery). Accepts `0`, `false`, `no`, `off` to
disable; anything else (or unset) enables it. Default: **enabled**.

Why disable: two processes sharing one identity (the "app-root split") must
not both announce the same `PeerId` over mDNS at once. They still reach each
other via Kademlia/bootstrap + dialing. The CLI node keeps it on; embedded
apps that share an identity across processes should call
`Config::with_mdns(false)`.

## `CANOPEE_LISTEN_PORT`

TCP port the swarm listens on. Default `0` (ephemeral). Set a fixed port to
be dialable at a known address (e.g. the public relay node uses `4001`).

## `CANOPEE_DEVICE_NAME`

The human-friendly name a device registers under its identity. Used by
`canopee device`. Defaults to the machine host name.

## `CANOPEE_IDENTITY_PASS`

If set, the identity key is stored password-encrypted at rest and unlocked
when the node starts. Unset by default (plaintext key file).

## `CANOPEE_BOOTSTRAP_ADDRS`

Replaces the default bootstrap address list (comma-separated multiaddrs).
The default is the single public relay
`/ip4/89.127.234.35/tcp/4001/p2p/12D3KooWEHGyyuEeLxfgmBPnutrCcz93nEmMswjuWcfhxBvxjbng`.
Point this at your own relay (see [Deployment](deployment.md)) to run a
private network.

## `CANOPEE_BOOTSTRAP_ADDRS_PREPEND`

Multiaddrs to add **before** the defaults — bootstrap your own relay *and*
keep the public one as a backstop.

## `CANOPEE_AUTO_RELAY`

Whether a fresh node automatically asks its bootstrap relays for a circuit
reservation on startup (so it is reachable through the relay, and dcutr can
upgrade the path to a direct connection later). Accepts `0`, `false`, `no`,
`off` to disable; anything else (or unset) enables it. Default: **enabled**.
Disable for embedded/app-root nodes that should never reserve relay circuits.

## `CANOPEE_EDGE_ADDR`

The libp2p multiaddr of the edge your node publishes through, e.g.
`/ip4/<edge-ip>/tcp/4002/p2p/<edge-peer-id>`. Optional: `canopee publish`
defaults to the built-in public bootstrap relay, which runs the edge role
like every node. Set this to publish through a different edge (e.g. your own
domain's node). See
[Publishing static apps](../guides/publishing-apps.md#serve-it-to-the-public-internet-through-an-edge).

## `CANOPEE_PUBLIC_BASE_DOMAIN`

The base domain the edge serves publishers under — a published app is
reachable at `https://<app-manifest-hash>.<CANOPEE_PUBLIC_BASE_DOMAIN>/`.
Default: `canopee.network`. Used by `canopee publish` to print the public
URL; it must match the DNS zone the edge operator actually serves.

## Edge role (every node)

Every `canopee-node` doubles as a publishing edge unless disabled — this is
how the public bootstrap relay serves publishers with no separate gateway:

* `CANOPEE_EDGE` — set to `0`, `false`, `no`, or `off` to disable the edge
  role. Default: **enabled**.
* `CANOPEE_EDGE_HTTP_PORT` — public HTTP(S) port browsers connect to.
  Default `8080`.
* `CANOPEE_EDGE_TLS_CERT` / `CANOPEE_EDGE_TLS_KEY` — PEM certificate + key.
  When both are set, the edge terminates TLS on the HTTP port; unset, it
  serves plain HTTP (put a TLS terminator in front).

## Standalone edge (`canopee-edge`) variables

The dedicated edge binary runs the same role as its own process (see
[Deploying an edge](deployment.md#deploying-an-edge)) and additionally
reads:

* `CANOPEE_EDGE_LISTEN_PORT` — libp2p TCP listen port. Default `4002`.
* `CANOPEE_EDGE_ROOT` — directory holding the edge's persistent keypair (so
  its peer id is stable across restarts). Default `~/.canopee-edge`.

See also [`crates/canopee-config`]'s `Config` for the programmatic
equivalents (`with_root`, `with_app_root`, `with_user_root`, `with_roots`,
`with_mdns`).