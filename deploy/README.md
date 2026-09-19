# Deploying Canopee services

Two systemd units:

* `canopee-node.service` — a public relay/bootstrap node for NAT'd peers.
  **It is also the public publishing edge**: every node runs the edge role
  by default, so once this service is up, `canopee publish` works through
  it with no further setup.
* `canopee-edge.service` — a *dedicated* edge gateway (its own peer id,
  stores nothing), for operators who want the gateway as a separate process
  (see the edge section at the end). Not needed for a normal relay.

## Deploying a relay node

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

## The relay is also the edge

The same `canopee-node` process serves `canopee publish` traffic: browsers
hit `CANOPEE_EDGE_HTTP_PORT` (default `8080`) and are tunneled to the
publisher. Publishers need nothing but the relay's normal multiaddr (the CLI
defaults to it). To serve HTTPS on your own domain:

1. Add a wildcard DNS record: `*.<base-domain>  A  <vps-ip>`.
2. Either set `CANOPEE_EDGE_TLS_CERT` + `CANOPEE_EDGE_TLS_KEY` (wildcard PEM)
   in the unit file and `CANOPEE_EDGE_HTTP_PORT=443`, or leave TLS off and
   reverse-proxy port 8080 from nginx/Caddy (which handles certs for you).
3. Tell publishers `CANOPEE_PUBLIC_BASE_DOMAIN=<base-domain>` so the CLI
   prints the right URL.

## Deploying a dedicated edge (optional)

Installs `canopee-edge` as a systemd service: a standalone public HTTP(S)
gateway that serves `https://<app-manifest-hash>.<base-domain>/` by
tunneling browser requests to the publisher's node over libp2p. Only needed
when you want the gateway as its own process with its own peer id — a normal
relay node already has this role built in (see above).

### Build

```bash
cargo build --release -p canopee-edge
```

### DNS

Publishers are served on per-app subdomains (a hash of the app manifest), so
the base domain needs a wildcard record pointing at this machine:

```
*.<base-domain>  A  <vps-ip>
```

### Install

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

### Configuration

* `CANOPEE_EDGE_LISTEN_PORT` (default `4002`) — libp2p port publishers dial
  to register. Open it in the firewall (`ufw allow 4002/tcp`). Keep it fixed
  so the edge's multiaddr is stable; the edge's keypair persists under
  `CANOPEE_EDGE_ROOT` (default `~/.canopee-edge`), so its peer id survives
  restarts too.
* `CANOPEE_EDGE_HTTP_PORT` (default `8080`) — browser-facing port.
* `CANOPEE_EDGE_TLS_CERT` + `CANOPEE_EDGE_TLS_KEY` — wildcard PEM cert/key;
  set both to terminate TLS on the edge itself, or leave unset and front it
  with nginx/Caddy (the edge then serves plain HTTP on the HTTP port).
* `CANOPEE_BOOTSTRAP_ADDRS` — point at the same relay/bootstrap your
  publishers use so the edge joins the same swarm (it validates registrations
  by fetching the signed app manifest from the registering peer).

Startup prints the address publishers need:

```text
Edge peer id: 12D3KooW…
Edge listens on: /ip4/0.0.0.0/tcp/4002/p2p/12D3KooW…
Edge serving http on 8080
```

Tell your publishers:

```bash
export CANOPEE_EDGE_ADDR=/ip4/<vps-ip>/tcp/4002/p2p/12D3KooW…
export CANOPEE_PUBLIC_BASE_DOMAIN=<base-domain>
canopee publish ./site
```

Their apps appear at `https://<app-manifest-hash>.<base-domain>/` — the URL
is derived from the content, so there is nothing to claim and nobody can
take an address that isn't theirs.
