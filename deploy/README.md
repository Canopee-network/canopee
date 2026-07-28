# Deploying a relay node

Installs `canopee-node` as a systemd service listening on a fixed TCP port,
suitable for a public VPS acting as a relay for NAT'd peers.

## Build

```bash
cargo build --release -p canopee-node -p canopee-cli
```

## Install

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

`CANOPEE_LISTEN_PORT` (set in the unit file, default `4001`) pins the libp2p
TCP listen port — required so the relay's multiaddr doesn't change across
restarts. Without it, `canopee-config` falls back to an ephemeral port
(`tcp/0`), which is fine for local dev but useless for a stable relay address.

Open this port in your firewall (e.g. `ufw allow 4001/tcp`).

## Get the relay's peer ID

```bash
sudo -u canopee /home/canopee/canopee/target/release/canopee-cli identity
```

## Point peers at the relay

```bash
canopee dial /ip4/<vps-ip>/tcp/4001/p2p/<relay-peer-id>
canopee listen-via-relay /ip4/<vps-ip>/tcp/4001/p2p/<relay-peer-id>
```
