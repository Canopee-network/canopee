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
`/ip4/89.127.234.35/tcp/4001/p2p/12D3KooWGiPk75fg8HBW7WJCouTTTLNi8W3s48sBK8AKewZKbCjC`.
Point this at your own relay (see [Deployment](deployment.md)) to run a
private network.

## `CANOPEE_BOOTSTRAP_ADDRS_PREPEND`

Multiaddrs to add **before** the defaults — bootstrap your own relay *and*
keep the public one as a backstop.

See also [`crates/canopee-config`]'s `Config` for the programmatic
equivalents (`with_root`, `with_app_root`, `with_user_root`, `with_roots`,
`with_mdns`).