use crate::message::{Hello, NetworkMessage, Welcome};
use crate::peer::Peer;
use crate::protocol::ProtocolVersion;
use canopee_identity::Identity;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub enum ConnectionState {
    Handshaking,
    Connected,
    Closing,
}

pub struct Connection {
    stream: TcpStream,
    identity: Arc<Identity>,
    state: ConnectionState,
    peer: Option<Peer>,
}

/* TCP handshake

Node A                    Node B
   |                        |
TCP connect                 |
   |                        |
   |                        |
   +---- Hello ------------>|
   |                        |
   <--- Welcome ------------|
   |                        |
connected                   |
 */

impl Connection {
    pub fn new(stream: TcpStream, identity: Arc<Identity>) -> Self {
        Self {
            stream,
            identity,
            state: ConnectionState::Handshaking,
            peer: None,
        }
    }

    async fn message_loop(&mut self) -> anyhow::Result<()> {
        println!("Peer connected");
        while let Ok(message) = self.receive_message().await {
            println!("Received: {:?}", message);
            self.handle_message(message).await?;
        }
        // loop {
        //     match self.receive_message().await {
        //         Ok(message) => {
        //         }
        //         Err(_) => break,
        //     }
        // }
        println!("Peer disconnected");
        Ok(())
    }

    async fn handle_message(&mut self, message: NetworkMessage) -> anyhow::Result<()> {
        match message {
            NetworkMessage::Ping => self.send_message(&NetworkMessage::Pong).await?,
            // NetworkMessage::...
            // NetworkMessage::...
            // ...
            message => println!("{:?}", message),
        }

        Ok(())
    }

    // -----

    pub async fn send_message(&mut self, message: &NetworkMessage) -> anyhow::Result<()> {
        let bytes = bincode::serialize(message)?;
        self.stream.write_u32(bytes.len() as u32).await?;
        self.stream.write_all(&bytes).await?;
        Ok(())
    }

    pub async fn receive_message(&mut self) -> anyhow::Result<NetworkMessage> {
        let size = self.stream.read_u32().await?;
        let mut buffer = vec![0; size as usize];
        self.stream.read_exact(&mut buffer).await?;
        Ok(bincode::deserialize(&buffer)?)
    }

    pub async fn send_hello(&mut self) -> anyhow::Result<()> {
        let protocol = ProtocolVersion::version();
        let hello = NetworkMessage::Hello(Hello {
            protocol,
            identity: self.identity.id().clone(),
        });
        self.send_message(&hello).await?;

        Ok(())
    }

    pub async fn send_welcome(&mut self) -> anyhow::Result<()> {
        let protocol = ProtocolVersion::version();
        let welcome = NetworkMessage::Welcome(Welcome {
            protocol,
            accepted: true,
            identity: self.identity.id().clone(),
        });
        self.send_message(&welcome).await?;

        Ok(())
    }

    //------

    pub async fn start_outbound(&mut self) -> anyhow::Result<()> {
        self.send_hello().await?;

        match self.receive_message().await? {
            NetworkMessage::Welcome(welcome) => {
                println!("Connected to {}", welcome.identity);
                self.state = ConnectionState::Connected;
                self.peer = Some(Peer {
                    identity: welcome.identity,
                    connected: true,
                    protocol_version: welcome.protocol,
                });

                let _ = self.message_loop().await;
            }
            message => {
                anyhow::bail!("Expected Welcome, got {:?}", message);
            }
        }
        Ok(())
    }

    pub async fn start_inbound(&mut self) -> anyhow::Result<()> {
        if let NetworkMessage::Hello(hello) = self.receive_message().await? {
            match ProtocolVersion::compatible(hello.protocol) {
                true => {
                    println!("Hello from {}", hello.identity);
                    self.send_welcome().await?;
                }
                false => anyhow::bail!("Protocol non compatible"),
            }
        }
        self.message_loop().await
    }
}
