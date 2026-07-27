use crate::connection::Connection;
use canopee_identity::Identity;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};

pub struct NetworkManager {
    identity: Arc<Identity>,
}

impl NetworkManager {
    pub fn new(identity: Arc<Identity>) -> Self {
        Self { identity }
    }

    pub async fn connect(&self, addr: &str) -> anyhow::Result<()> {
        let stream = TcpStream::connect(addr).await?;
        let mut connection = Connection::new(stream, self.identity.clone());
        tokio::spawn(async move {
            if let Err(e) = connection.start_outbound().await {
                eprintln!("Connection failed: {e}");
            }
        });

        Ok(())
    }

    pub async fn listen(&self, addr: &str) -> anyhow::Result<()> {
        let listener = TcpListener::bind(addr).await?;
        loop {
            let (stream, _) = listener.accept().await?;
            let mut connection = Connection::new(stream, self.identity.clone());
            tokio::spawn(async move {
                if let Err(e) = connection.start_inbound().await {
                    eprintln!("Accept failed: {e}");
                }
            });
        }
    }
}
