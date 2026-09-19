use crate::message::{
    CanopeePairingRequest, CanopeePairingResponse, ObjectRequest, ObjectResponse, ServeRequest,
    ServeRegistration, ServeRegistrationResponse, ServeResponse,
};
use libp2p::{
    autonat, dcutr, gossipsub, identify, kad, mdns, ping, relay, request_response,
    swarm::behaviour::toggle::Toggle,
    swarm::NetworkBehaviour,
};

pub const IDENTIFY_PROTOCOL: &str = "/canopee/id/1.0.0";
pub const KAD_PROTOCOL: &[u8] = b"/canopee/kad/1.0.0";
/// The LAN device-pairing request/response protocol (Phase 3).
pub const PAIRING_PROTOCOL: &str = "/canopee/pairing/1.0.0";
/// Publisher -> edge (or edge <-> publisher over an established connection):
/// the edge forwards HTTP requests for `username.domain/...` to the publisher
/// node whose connection carries the `username` subdomain.
pub const SERVE_PROTOCOL: &str = "/canopee/serve/1.0.0";
/// Publisher -> edge: a signed registration claiming this connection serves a
/// username, so the edge can pin the subdomain and forward requests here.
pub const SERVE_REGISTRY_PROTOCOL: &str = "/canopee/serve-registry/1.0.0";

pub type ObjectExchange = request_response::cbor::Behaviour<ObjectRequest, ObjectResponse>;
pub type PairingExchange =
    request_response::cbor::Behaviour<CanopeePairingRequest, CanopeePairingResponse>;
pub type ServeExchange = request_response::cbor::Behaviour<ServeRequest, ServeResponse>;
pub type ServeRegistryExchange =
    request_response::cbor::Behaviour<ServeRegistration, ServeRegistrationResponse>;

#[derive(NetworkBehaviour)]
pub struct CanopeeBehaviour {
    pub identify: identify::Behaviour,
    pub kad: kad::Behaviour<kad::store::MemoryStore>,
    pub object_exchange: ObjectExchange,
    pub pairing: PairingExchange,
    pub serve: ServeExchange,
    pub serve_registry: ServeRegistryExchange,
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
