# Bootstrap & relay nodes

Two related jobs, both built into every `canopee-node`:

- **Bootstrap nodes** give a brand-new node its *first* entries in the
  Kademlia routing table, so it can find peers and objects without being
  handed an address — bootstrap is a door, not a dependency (after the
  first few peers, discovery works through the DHT itself).
- **Relay nodes** (circuit relay) let nodes behind NAT connect to each other
  through a publicly reachable peer. Every node can relay by default; a
  public node just holds reservations for NAT'd peers.

Both come for free with a stock `canopee-node` — a public node doesn't run
special software, it just has a public address and a fixed port.

## The built-in defaults

A fresh node dials the built-in bootstrap list on startup. The default is a
single entry (in `bootstrap_addrs()` in
`crates/canopee-network/src/manager.rs`):

```
/ip4/89.127.234.35/tcp/4001/p2p/12D3KooWEHGyyuEeLxfgmBPnutrCcz93nEmMswjuWcfhxBvxjbng
```

After connecting, the existing `ConnectionEstablished` handler registers the
peer with Kademlia automatically — nothing extra to wire. And since
`canopee-network` now requests a circuit reservation on each bootstrap relay
by default (see `CANOPEE_AUTO_RELAY`), a NAT'd node is *also* reachable
through the relay without running `listen-via-relay` by hand.

## Run your own relay

1. Install `canopee-node` as a systemd service on a public VPS, with a
   **fixed port** — see [Deployment](../reference/deployment.md):

   ```
   CANOPEE_LISTEN_PORT=4001
   ```

2. Get its peer id:

   ```bash
   canopee identity   # → canopee://identity/12D3KooW...Relay
   ```

3. Point peers at it:

   ```bash
   canopee dial             /ip4/<vps-ip>/tcp/4001/p2p/12D3KooW...Relay
   canopee listen-via-relay /ip4/<vps-ip>/tcp/4001/p2p/12D3KooW...Relay
   ```

## Run a private network

Nodes join through bootstrap addresses. To keep a test network (or a
closed deployment) self-contained, replace the public default:

```bash
CANOPEE_BOOTSTRAP_ADDRS=/ip4/<vps-ip>/tcp/4001/p2p/<relay-peer-id> canopee start
```

Or prepend your own relay while keeping the public one as a fallback:

```bash
CANOPEE_BOOTSTRAP_ADDRS_PREPEND=/ip4/<vps-ip>/tcp/4001/p2p/<relay-peer-id> canopee start
```

Full environment reference: [Environment](../reference/environment.md).

## Verifying bootstrap worked

```bash
canopee status           # reports peer count once up
canopee peers            # list observed peers (should include bootstrap relay)
canopee relay-status     # your circuit reservations
```

A node can discover providers and records through the DHT even when it can't
dial a bootstrap peer directly — bootstrap is only the first step in.

## NAT coverage recap

1. mDNS finds *LAN* peers automatically.
2. Bootstrap addresses join you to the global DHT.
3. `listen-via-relay` makes you reachable via a relay.
4. dcutr silently upgrades relayed connections to direct ones when hole
   punching succeeds.

See [Networking concept](../concepts/networking.md) for the full model.