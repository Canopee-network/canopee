use canopee_identity::IdentityId;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Debug, Serialize, Deserialize)]
pub struct Peer {
    pub identity: IdentityId,
    pub address: SocketAddr,
    pub connected: bool,
    pub last_seen: u64,
    pub protocol_version: u32,
}
