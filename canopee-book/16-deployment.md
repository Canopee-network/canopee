# Chapter 16: Deployment

## Deployment Models

Canopee supports several deployment models, from development to production relay nodes.

## Development Mode

For development and testing, run everything locally:

```bash
# Initialize
canopee init

# Start the node
canopee start

# Use it
canopee put ./myfile.txt
canopee peers
```

The node runs as a background process, accessible via the Unix socket. Multiple nodes can run on the same machine with different `HOME` directories:

```bash
HOME=/tmp/node-a canopee init && HOME=/tmp/node-a canopee start &
HOME=/tmp/node-b canopee init && HOME=/tmp/node-b canopee start &
```

## Embedded Desktop Application

For desktop applications (Tauri, Electron, native), embed the runtime in the application process:

```rust
// In your app's initialization code
let config = Config::new()
    .with_app_root(app_data_dir)
    .with_user_root(user_data_dir)
    .with_mdns(false);

let runtime = Runtime::open_with_config(config).await?;
```

The application manages the runtime's lifecycle. No daemon, no socket, no external process.

## Relay Node Deployment

Relay nodes are public-facing peers that provide relay services for peers behind NATs. Deploy a relay node with systemd:

### Build

```bash
cargo build --release -p canopee-node -p canopee-cli
```

### Install

```bash
# Copy binaries (matching the layout the systemd unit expects)
mkdir -p /home/canopee/canopee/target/release
cp target/release/canopee-node /home/canopee/canopee/target/release/
cp target/release/canopee-cli /home/canopee/canopee/target/release/
```

### systemd Unit

The `deploy/canopee-node.service` unit (note: no `WorkingDirectory`; the binary path is under `target/release`):

```ini
[Unit]
Description=Canopee Relay Node
After=network.target

[Service]
Type=simple
User=canopee
ExecStart=/home/canopee/canopee/target/release/canopee-node
Environment=CANOPEE_LISTEN_PORT=4001
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

### Configure

```bash
# Initialize the relay's identity (init only creates the identity; the port
# belongs in the systemd unit's Environment, as shown above)
sudo -u canopee /home/canopee/canopee/target/release/canopee-cli init

# Start the relay
sudo systemctl enable --now canopee-relay

# Get the relay's peer ID
sudo -u canopee /home/canopee/canopee/target/release/canopee-cli identity
```

### Connect to the Relay

Other peers connect to the relay:

```bash
canopee dial /ip4/RELAY_IP/tcp/4001/p2p/RELAY_PEER_ID
canopee listen-via-relay /ip4/RELAY_IP/tcp/4001/p2p/RELAY_PEER_ID
```

## NAT Traversal

Peers behind NATs use two mechanisms:

### Relay

Connect through a relay node:

```bash
canopee listen-via-relay /ip4/relay.example.com/tcp/4001/p2p/12D3KooWRELAY...
```

The swarm requests a circuit reservation. Traffic flows through the relay until a direct connection is established.

### Hole Punching (DCUtR)

Once connected through a relay, the swarm uses DCUtR to establish a direct connection:

1. Both peers exchange their observed addresses through the relay
2. Both peers simultaneously attempt to connect to each other's observed addresses
3. If the NAT mapping is permissive, the direct connection succeeds
4. The relay connection is dropped

DCUtR works with NATs that use port preservation or predictable port allocation. It does not work with symmetric NATs — those require permanent relay usage.

## Network Topology

A typical Canopee network looks like:

```
    [Relay Node] ← public, stable address
         ↕
    [Peer A] ← behind NAT, connects via relay
    [Peer B] ← behind NAT, connects via relay
         ↕
    [Peer C] ← on LAN, discovered via mDNS
    [Peer D] ← on LAN, discovered via mDNS
```

- **Relay nodes**: public-facing, stable address, provide relay services
- **NAT peers**: connect to relays, use DCUtR for direct connections
- **LAN peers**: discovered automatically via mDNS

## Bootstrap Strategy

New nodes automatically dial a set of default bootstrap addresses on startup to seed the Kademlia routing table. A default bootstrap relay is hardcoded in the network crate and can be overridden:

- `CANOPEE_BOOTSTRAP_ADDRS` — replace the default bootstrap address list
- `CANOPEE_BOOTSTRAP_ADDRS_PREPEND` — prepend additional addresses

Additional ways to join:

1. **Automatic bootstrap**: nodes dial the bootstrap addresses on startup (no action needed)
2. **Manual dial**: `canopee dial /ip4/KNOWN_PEER/tcp/PORT/p2p/PEER_ID`
3. **Relay connection**: `canopee listen-via-relay /ip4/RELAY/tcp/PORT/p2p/RELAY_ID`
4. **LAN discovery**: mDNS finds local peers automatically

## Monitoring

The node exposes status via the CLI:

```bash
canopee status    # running/stopped, identity, object count, peer count
canopee peers     # connected peers with usernames
```

`status` reports identity, object count, and peer count (there is no uptime metric). Output is largely human-readable; for production monitoring, a structured logging layer would be a future addition.

## Backup and Recovery

### Identity Backup

The identity key is the most critical file. Back it up:

```bash
cp ~/.canopee/identity/identity.key /secure/backup/
```

If the key is encrypted, also back up the passphrase. Without the key, the identity is lost forever.

### Object Store Backup

The object store is a flat directory of content-addressed files. Back it up like any other directory:

```bash
tar -czf canopee-storage.tar.gz ~/.canopee/storage/
```

### Records Backup

```bash
tar -czf canopee-records.tar.gz ~/.canopee/records/
```

Records can be reconstructed from the DHT (if published), but local copies are authoritative and faster.

## Scaling Considerations

### Object Store

The flat-file storage is O(n) for listing and has no indexing. For stores with millions of objects, a future improvement would be a SQLite index or similar.

### DHT

Kademlia DHTs scale logarithmically with network size. A Canopee network with millions of peers would have O(log n) lookup hops — well within practical limits.

### Relay Load

Every node relays traffic for others. For networks with many NAT peers, dedicated relay nodes with higher bandwidth and stable connections are recommended.

## Security Checklist

- [ ] Identity key encrypted at rest (if sensitive)
- [ ] Identity key backed up securely
- [ ] Relay nodes on trusted infrastructure
- [ ] No sensitive data stored as shared objects
- [ ] Application-level E2E encryption for confidential data
- [ ] Firewall rules for relay nodes (only expose the Canopee port)
