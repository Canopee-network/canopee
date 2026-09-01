use crate::message::{ObjectRequest, ObjectResponse};
use libp2p::{
    autonat, dcutr, gossipsub, identify, kad, mdns, ping, relay, request_response,
    swarm::behaviour::toggle::Toggle,
    swarm::NetworkBehaviour,
};

pub const IDENTIFY_PROTOCOL: &str = "/canopee/id/1.0.0";
pub const KAD_PROTOCOL: &[u8] = b"/canopee/kad/1.0.0";

pub type ObjectExchange = request_response::cbor::Behaviour<ObjectRequest, ObjectResponse>;

#[derive(NetworkBehaviour)]
pub struct CanopeeBehaviour {
    pub identify: identify::Behaviour,
    pub kad: kad::Behaviour<kad::store::MemoryStore>,
    pub object_exchange: ObjectExchange,
    pub ping: ping::Behaviour,
    /// Disabled when a `NetworkManager` is created with `mdns: false` (the
    /// embedded-app default): a shared identity should never be *announced*
    /// over mDNS, or a second app's swarm writing the same `PeerId` would
    /// fight the first. Disabled toggles still own the socket but never
    /// poll, so they neither discover nor are discovered.
    pub mdns: Toggle<mdns::tokio::Behaviour>,
    pub gossipsub: gossipsub::Behaviour,
    pub relay: relay::Behaviour,
    pub relay_client: relay::client::Behaviour,
    pub dcutr: dcutr::Behaviour,
    pub autonat: autonat::Behaviour,
}
