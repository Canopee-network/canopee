use canopee_config::Config;
use canopee_protocol::{NodeCommand, NodeResponse};
use serde::Deserialize;
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
        Ok(Self { socket })
    }

    async fn send(&self, stream: &mut UnixStream, bytes: &[u8]) -> anyhow::Result<()> {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(bytes).await?;
        Ok(())
    }

    async fn receive<T: for<'a> Deserialize<'a>>(
        &self,
        stream: &mut UnixStream,
    ) -> anyhow::Result<T> {
        let size = stream.read_u32().await?;
        let mut buffer = vec![0u8; size as usize];
        stream.read_exact(&mut buffer).await?;
        Ok(bincode::deserialize(&buffer)?)
    }

    pub async fn request(&self, command: NodeCommand) -> anyhow::Result<NodeResponse> {
        let mut stream = UnixStream::connect(&self.socket).await?;
        let bytes = bincode::serialize(&command)?;
        self.send(&mut stream, &bytes).await?;
        self.receive(&mut stream).await
    }

    pub async fn is_running(&self) -> bool {
        UnixStream::connect(&self.socket).await.is_ok()
    }

    // move here, from cli:
    // put()
    // get()
    // list()
    // status()
    // export()
    // import()
    // ...
}
