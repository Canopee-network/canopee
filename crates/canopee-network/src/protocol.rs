/*
 * TODO
 * NOT YET USED!!
 */
pub const PROTOCOL_VERSION: u32 = 1;
/*
 * TODO
 * NOT YET USED!!
 */
pub const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024; // 16 MB
/*
 * TODO
 * NOT YET USED!!
 */
pub const NODE_PROTOCOL: &str = "canopee";
/*
 * TODO
 * NOT YET USED!!
 */
#[derive(Debug, Clone)]
pub struct ProtocolVersion {
    pub major: u32,
    pub minor: u32,
}
/*
 * TODO
 * NOT YET USED!!
 */
impl ProtocolVersion {
    pub fn current() -> Self {
        Self { major: 1, minor: 0 }
    }

    pub fn version() -> u32 {
        PROTOCOL_VERSION
    }

    pub fn compatible(version: u32) -> bool {
        version == PROTOCOL_VERSION
    }
}

/*
*
NetworkMessage::Hello(hello) => {
    if !protocol::compatible(hello.protocol) {
        return Err(anyhow!("Unsupported protocol"));
    }

}

protocol upgrades
encrypted connections
peer authentication
object replication
NAT traversal
compatibility between different Canopee versions
...

*/
