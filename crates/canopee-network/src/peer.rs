use canopee_identity::IdentityId;
use libp2p::{Multiaddr, PeerId};

#[derive(Debug, Clone)]
pub struct Peer {
    pub peer_id: PeerId,
    pub identity: Option<IdentityId>,
    pub addresses: Vec<Multiaddr>,
}

impl Peer {
    pub fn new(peer_id: PeerId) -> Self {
        Self {
            peer_id,
            identity: None,
            addresses: Vec::new(),
        }
    }
}
