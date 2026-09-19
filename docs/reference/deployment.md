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

It is also — automatically — the **public publishing edge**. Every node runs
the edge role unless disabled (`CANOPEE_EDGE=0`): it validates publisher
registrations and proxies browser HTTP(S) to them on `CANOPEE_EDGE_HTTP_PORT`
(default `8080`; TLS via `CANOPEE_EDGE_TLS_CERT`/`CANOPEE_EDGE_TLS_KEY`, or a
TLS terminator in front). That is why `canopee publish` needs no
`CANOPEE_EDGE_ADDR`: the CLI defaults to this relay's multiaddr. To serve
HTTPS on a nice domain, give the relay a DNS wildcard (`*.<base-domain>`)
plus a wildcard cert, exactly as described for the standalone edge below —
the configuration variables are the same.

## Deploying an edge

An **edge** is the public web gateway that makes `canopee publish` work: it
owns a DNS wildcard (`*.<base-domain>`) and terminates browser traffic,
tunneling each request over libp2p to the node that registered the matching
app-hash subdomain.

You usually do **not** need a separate edge: every `canopee-node` already
runs the edge role (see above — that is what the public bootstrap relay
uses). Run the standalone `canopee-edge` binary instead when you want a
dedicated gateway process with its own peer id that stores no objects — it
joins the same DHT to validate publishers but hosts no content of its own.

### Build

```bash
cargo build --release -p canopee-edge
```

### DNS

Create a wildcard A record so every publisher subdomain reaches the edge:

```
*.<base-domain>  A  <edge-ip>
```

### Install as a service

The unit file is `deploy/canopee-edge.service`:

```bash
useradd -m -s /bin/bash canopee
mkdir -p /home/canopee/canopee/target/release
cp target/release/canopee-edge /home/canopee/canopee/target/release/
chown -R canopee:canopee /home/canopee/canopee

cp deploy/canopee-edge.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now canopee-edge
journalctl -u canopee-edge -f
```

Configuration (all optional, see
[Environment variables](environment.md#edge-role-every-node)):

- `CANOPEE_EDGE_LISTEN_PORT` (default `4002`) — libp2p port publishers dial
  to register. Open it in the firewall (`ufw allow 4002/tcp`).
- `CANOPEE_EDGE_HTTP_PORT` (default `8080`) — browser-facing HTTP(S) port.
- `CANOPEE_EDGE_TLS_CERT` / `CANOPEE_EDGE_TLS_KEY` — PEM cert+key for your
  wildcard (`*.<base-domain>`); when both are set the edge terminates TLS
  itself. Without them it serves plain HTTP on `CANOPEE_EDGE_HTTP_PORT` —
  front it with any TLS terminator (nginx, Caddy, a LB) in that case.
- `CANOPEE_EDGE_ROOT` — where the edge's persistent keypair lives (default
  `~/.canopee-edge`). Keep it: a stable keypair means a stable peer id, so
  publishers don't have to update `CANOPEE_EDGE_ADDR`.

Startup prints parseable lines — grab the address publishers should use:

```text
Edge peer id: 12D3KooW…
Edge listens on: /ip4/0.0.0.0/tcp/4002/p2p/12D3KooW…
Edge serving https on 443
```

Tell publishers to set:

```bash
export CANOPEE_EDGE_ADDR=/ip4/<edge-ip>/tcp/4002/p2p/12D3KooW…
export CANOPEE_PUBLIC_BASE_DOMAIN=<base-domain>
```

### Trust model, briefly

The edge is a **relay, not an owner**: a publisher registers an app with a
signed claim naming the app manifest's hash; the edge fetches the manifest
from the registering peer, verifies it hashes to the claimed id, and checks
its `owner` is the identity that signed the claim. Because the address *is*
a content hash, there is no namespace to squat on — nobody can register
someone else's app, full stop. Registrations expire after 3 missed
heartbeats (~90s), so a crashed publisher goes dark automatically, and the
edge stores no app content of its own. Note that, like any reverse proxy,
the edge *does* see the plaintext HTTP traffic it forwards — publishers who
need end-to-end confidentiality should treat the edge as a transport, not a
trust boundary.