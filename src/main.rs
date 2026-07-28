fn main() {
    println!("Hello, world!");
    // public testing relay
    // /ip4/89.127.234.35/tcp/4001/p2p/12D3KooWGiPk75fg8HBW7WJCouTTTLNi8W3s48sBK8AKewZKbCjC
    //
    // How to test the chat between two peers through the relay
    // On your VPS relay: already running per deploy/README.md, listening on port 4001. Get its peer ID:
    // sudo -u canopee /home/canopee/canopee/target/release/canopee-cli identity

    // On each of the two peer machines:
    // cargo run -p canopee-cli -- start     # or run canopee-node directly
    // cargo run -p canopee-cli -- identity  # note this peer's ID
    // cargo run -p canopee-cli -- listen-via-relay /ip4/<vps-ip>/tcp/4001/p2p/<relay-peer-id>

    // Peer A dials Peer B through the relay (this triggers the reservation handshake and dcutr hole-punch attempt):
    // cargo run -p canopee-cli -- dial /ip4/<vps-ip>/tcp/4001/p2p/<relay-peer-id>/p2p-circuit/p2p/<peer-B-id>
    // Watch journalctl -u canopee-node -f (or your local node's stdout) on both peers for Hole punch event: ... — that confirms dcutr attempted/succeeded at upgrading to a direct connection.

    // Both peers, chat:
    // cargo run -p canopee-cli -- chat general
    // Type a line and hit enter to send; incoming messages from the other peer print as <peer-id>: <message>.
}
