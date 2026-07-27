use canopee_identity::IdentityId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NodeState {
    pub identity: IdentityId,
    pub created_at: u64,
    pub last_started_at: Option<u64>,
    pub started: bool,
    pub version: u32,
    pub peers: Vec<String>,
}
