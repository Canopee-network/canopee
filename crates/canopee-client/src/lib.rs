use canopee_config::Config;
use canopee_protocol::{NodeCommand, NodeResponse};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

pub struct NodeClient {
    socket: PathBuf,
}

impl NodeClient {
    pub async fn new() -> anyhow::Result<Self> {
        let config = Config::new();
        let socket = config.node_socket_path();
        if !socket.exists() {
            anyhow::bail!("Canopee node is not running");
        }

        Ok(Self { socket })
    }
    pub async fn request(&self, command: NodeCommand) -> anyhow::Result<NodeResponse> {
        let mut stream = UnixStream::connect(&self.socket).await?;
        let bytes = bincode::serialize(&command)?;
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await?;
        let size = stream.read_u32().await?;
        let mut buffer = vec![0u8; size as usize];
        stream.read_exact(&mut buffer).await?;
        let response = bincode::deserialize(&buffer)?;

        Ok(response)
    }
    pub async fn is_running(&self) -> bool {
        UnixStream::connect(&self.socket).await.is_ok()
    }
}
