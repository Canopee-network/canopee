use canopee_config::Config;
use canopee_protocol::{NodeCommand, NodeResponse};
use serde::Deserialize;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// Low-level request/response transport to the local node over its Unix socket.
/// Most apps should use [`crate::CanopeeClient`] instead.
pub struct NodeClient {
    socket: PathBuf,
}

impl NodeClient {
    pub async fn new() -> anyhow::Result<Self> {
        let config = Config::new();
        let socket = config.node_socket_path();
        Ok(Self { socket })
    }

    pub(crate) async fn write_frame(stream: &mut UnixStream, bytes: &[u8]) -> anyhow::Result<()> {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(bytes).await?;
        Ok(())
    }

    pub(crate) async fn read_frame<T: for<'a> Deserialize<'a>>(
        stream: &mut UnixStream,
    ) -> anyhow::Result<T> {
        let size = stream.read_u32().await?;
        let mut buffer = vec![0u8; size as usize];
        stream.read_exact(&mut buffer).await?;
        Ok(bincode::deserialize(&buffer)?)
    }

    /// Like [`Self::read_frame`], but tells `None` (a clean close before any
    /// byte of the next frame) apart from a transport/decode failure.
    ///
    /// Note: only safe on an *established* stream (a subscription after its
    /// handshake), not on a freshly connected socket — tokio 1.x's
    /// `UnixStream` first `poll_read` can return a corrupt view on a brand
    /// new connection, which is why [`Self::request`] keeps the plain
    /// `read_u32` path above for one-shot request/response.
    pub(crate) async fn read_frame_opt<T: for<'a> Deserialize<'a>>(
        stream: &mut UnixStream,
    ) -> anyhow::Result<Option<T>> {
        // `read_u8` errors with UnexpectedEof iff zero bytes arrived — a clean
        // close at a frame boundary. Any later truncation still surfaces as an
        // error from `read_exact`.
        let first = match stream.read_u8().await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut header = [first, 0, 0, 0];
        stream.read_exact(&mut header[1..]).await?;
        let size = u32::from_le_bytes(header) as usize;
        let mut buffer = vec![0u8; size];
        stream.read_exact(&mut buffer).await?;
        Ok(Some(bincode::deserialize(&buffer)?))
    }

    pub async fn request(&self, command: NodeCommand) -> anyhow::Result<NodeResponse> {
        let mut stream = self.connect().await?;
        let bytes = bincode::serialize(&command)?;
        Self::write_frame(&mut stream, &bytes).await?;
        Self::read_frame(&mut stream).await
    }

    pub(crate) async fn connect(&self) -> anyhow::Result<UnixStream> {
        Ok(UnixStream::connect(&self.socket).await?)
    }

    pub async fn is_running(&self) -> bool {
        UnixStream::connect(&self.socket).await.is_ok()
    }
}
