# Deployment

The operational story: running the node daemon as a systemd service for a
long-lived public relay/bootstrap node. This is how the built-in public
relay (`/ip4/89.127.234.35/tcp/4001/...`) is operated.

The full install script is `deploy/README.md` + `deploy/canopee-node.service`
in the repo root. Summary:

## Build

```bash
cargo build --release -p canopee-node -p canopee-cli
```

## Install as a service

```bash
useradd -m -s /bin/bash canopee
mkdir -p /home/canopee/canopee/target/release
cp target/release/canopee-node /home/canopee/canopee/target/release/
chown -R canopee:canopee /home/canopee/canopee

cp deploy/canopee-node.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now canopee-node
journalctl -u canopee-node -f
```

## Configuration

- `CANOPEE_LISTEN_PORT` (set in the unit file, default `4001`) pins the
  libp2p TCP listen port — **required** so the relay's multiaddr doesn't
  change across restarts. Without it config falls back to an ephemeral port
  (`tcp/0`), fine for dev, useless for a stable relay address.
- Open the port in the firewall (`ufw allow 4001/tcp`).

## Get the relay's peer id

```bash
sudo -u canopee /home/canopee/canopee/target/release/canopee-cli identity
```

## Point peers at the relay

```bash
canopee dial             /ip4/<vps-ip>/tcp/4001/p2p/<relay-peer-id>
canopee listen-via-relay /ip4/<vps-ip>/tcp/4001/p2p/<relay-peer-id>
```

## Make it a bootstrap

Nodes join the DHT through their bootstrap addresses
(`CANOPEE_BOOTSTRAP_ADDRS`, `CANOPEE_BOOTSTRAP_ADDRS_PREPEND`). To let other
nodes discover *your* relay as a bootstrap, publish its multiaddr — or run a
private network where every node sets:

```
CANOPEE_BOOTSTRAP_ADDRS=/ip4/<vps-ip>/tcp/4001/p2p/<relay-peer-id>
```

See [Bootstrap & relay guide](../guides/bootstrap-nodes.md).

## What a relay node actually runs

A public relay node is just `canopee-node` with `CANOPEE_LISTEN_PORT=4001`:
the swarm ships relay support on by default (`canopee-network`'s relay
behaviour + autonat/dcutr), so a public node can circuit-relay for NAT'd
peers with no extra configuration. No special binary, no separate service.