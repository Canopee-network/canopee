use canopee_identity::IdentityId;
use libp2p::{Multiaddr, PeerId};

#[derive(Debug, Clone)]
pub struct Peer {
    pub peer_id: PeerId,
    pub identity: Option<IdentityId>,
    /// The peer's claimed, globally unique username (from its `(owner,
    /// "username")` record), resolved opportunistically when the peer is
    /// connected and the node can reach the DHT/store.
    pub username: Option<String>,
    /// The peer's profile display name (from its `(owner, "profile")`
    /// record), if one has been resolved.
    pub display_name: Option<String>,
    pub addresses: Vec<Multiaddr>,
}

impl Peer {
    pub fn new(peer_id: PeerId) -> Self {
        Self {
            peer_id,
            identity: None,
            username: None,
            display_name: None,
            addresses: Vec::new(),
        }
    }
}

/// An accepted circuit reservation on a relay, tracked so callers can confirm
/// the reservation succeeded (and see the resulting dialable addresses)
/// before asking others to dial us through it.
#[derive(Debug, Clone)]
pub struct RelayReservation {
    pub relay_peer_id: PeerId,
    /// Whether the most recent accepted request renewed an existing reservation.
    pub renewal: bool,
    /// Dialable `/p2p-circuit` addresses learned via `NewListenAddr` for this relay.
    pub listen_addrs: Vec<Multiaddr>,
}

impl RelayReservation {
    pub fn new(relay_peer_id: PeerId) -> Self {
        Self {
            relay_peer_id,
            renewal: false,
            listen_addrs: Vec::new(),
        }
    }
}
