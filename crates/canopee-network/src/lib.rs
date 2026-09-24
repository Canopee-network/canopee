pub mod behaviour;
pub mod manager;
pub mod message;
pub mod peer;

pub use libp2p::{Multiaddr, PeerId};
pub use manager::{
    InboundPairing, InboundServe, InboundServeRegistration, NetworkManager, ObjectProvider,
    ObjectStore, DEFAULT_BOOTSTRAP_ADDRS,
};
pub use message::{
    app_subdomain, CanopeePairingRequest, CanopeePairingResponse, ObjectRequest, ObjectResponse,
    PubSubMessage, ServeRegistration, ServeRegistrationResponse, ServeRequest, ServeResponse,
    SERVE_FORWARDED_HEADERS,
};
pub use peer::{Peer, RelayReservation};

/// Returns the peer id this multiaddr references: the last `/p2p/<id>` in the
/// path (e.g. for an edge address). `None` if the address names no peer.
pub fn peer_id_from_multiaddr(addr: &Multiaddr) -> Option<PeerId> {
    let mut last = None;
    for proto in addr.iter() {
        if let libp2p::multiaddr::Protocol::P2p(pid) = proto {
            last = Some(pid);
        }
    }
    last
}
