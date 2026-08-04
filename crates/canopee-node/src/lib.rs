use canopee_protocol::{NodeCommand, NodeResponse};
use canopee_runtime::Runtime;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;
use tokio::task::JoinSet;

#[derive(Clone)]
pub struct Node {
    runtime: Arc<Runtime>,
    shutdown: broadcast::Sender<()>,
}

impl Node {
    pub fn new(runtime: Runtime) -> Self {
        let (shutdown, _) = broadcast::channel(1);
        Self {
            runtime: Arc::new(runtime),
            shutdown,
        }
    }

    pub async fn open() -> anyhow::Result<Self> {
        let runtime = Runtime::open().await?;
        Ok(Self::new(runtime))
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        let socket_path = self.runtime.node_socket_path();
        if socket_path.exists() {
            tokio::fs::remove_file(&socket_path).await?;
        }
        let listener = UnixListener::bind(&socket_path)?;
        println!("Canopee node listening on {:?}", socket_path);
        let mut shutdown = self.shutdown.subscribe();
        let mut tasks: JoinSet<()> = JoinSet::new();
        self.runtime.mark_started().await?;

        loop {
            tokio::select! {
                result = listener.accept() => {
                    let (stream, _) = result?;
                    let node = self.clone();
                    tasks.spawn(async move {
                        if let Err(e) = node.handle_connection(stream).await {
                            eprintln!("Connection error: {}", e);
                        }
                    });
                }
                result = shutdown.recv() => {
                    println!("Shutdown received: {:?}", result);
                    tasks.abort_all();
                    while let Some(result) = tasks.join_next().await {
                        match result {
                            Ok(_) => {}
                            Err(e) => {
                                if !e.is_cancelled() {
                                    eprintln!("Task failed: {}", e);
                                }
                            }
                        }
                    }
                    break;
                }
            }
        }
        tasks.abort_all();
        if socket_path.exists() {
            tokio::fs::remove_file(&socket_path).await?;
        }

        Ok(())
    }

    async fn read_command(&self, stream: &mut UnixStream) -> anyhow::Result<NodeCommand> {
        println!("stream: {:? }", stream);
        let size = stream.read_u32().await?;
        let mut buffer = vec![0u8; size as usize];
        stream.read_exact(&mut buffer).await?;
        let command = bincode::deserialize(&buffer)?;

        Ok(command)
    }

    async fn write_response(
        &self,
        stream: &mut UnixStream,
        response: NodeResponse,
    ) -> anyhow::Result<()> {
        println!("response: {:? }", response);
        let bytes = bincode::serialize(&response)?;
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await?;

        Ok(())
    }

    async fn handle_connection(&self, mut stream: UnixStream) -> anyhow::Result<()> {
        let command: NodeCommand = self.read_command(&mut stream).await?;

        if let NodeCommand::Subscribe { topic } = command {
            return self.handle_subscribe(stream, topic).await;
        }

        let should_shutdown = matches!(command, NodeCommand::Shutdown);
        let response = self.handle(command).await;
        if let Err(e) = self.write_response(&mut stream, response).await {
            eprintln!("Failed writing response: {}", e);
        }
        if should_shutdown {
            println!("Sending shutdown signal");
            let _ = self.shutdown.send(());
        }
        Ok(())
    }

    async fn handle_subscribe(&self, mut stream: UnixStream, topic: String) -> anyhow::Result<()> {
        let mut receiver = match self.runtime.network.subscribe(&topic).await {
            Ok(receiver) => receiver,
            Err(e) => {
                let _ = self
                    .write_response(
                        &mut stream,
                        NodeResponse::Error {
                            message: e.to_string(),
                        },
                    )
                    .await;
                return Ok(());
            }
        };
        self.write_response(&mut stream, NodeResponse::Subscribed)
            .await?;

        let mut shutdown = self.shutdown.subscribe();
        loop {
            tokio::select! {
                message = receiver.recv() => {
                    let Ok(message) = message else { break };
                    if message.topic != topic {
                        continue;
                    }
                    let response = NodeResponse::PubSub(canopee_protocol::PubSubMessage {
                        topic: message.topic,
                        source: message.source.map(|p| p.to_string()),
                        data: message.data,
                    });
                    if self.write_response(&mut stream, response).await.is_err() {
                        break;
                    }
                }
                _ = shutdown.recv() => break,
            }
        }

        let _ = self.runtime.network.unsubscribe(&topic).await;
        Ok(())
    }

    pub async fn handle(&self, command: NodeCommand) -> NodeResponse {
        match command {
            NodeCommand::Identity => {
                let identity_id = self.runtime.identity.identity_id.clone();
                NodeResponse::Identity { identity_id }
            }

            NodeCommand::Put { data } => match self.runtime.put(data).await {
                Ok(id) => NodeResponse::ObjectCreated { id },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::Get { id } => match self.runtime.get(&id).await {
                Ok(object) => NodeResponse::Object { object },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::List => match self.runtime.list().await {
                Ok(objects) => NodeResponse::Objects { objects },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::Status => {
                let objects = self.runtime.list().await.unwrap_or_default().len();
                let peers = self.runtime.network.peers().await.unwrap_or_default().len();
                NodeResponse::Status {
                    identity: self.runtime.identity().id().to_string(),
                    objects,
                    peers,
                }
            }

            NodeCommand::Export { id } => match self.runtime.export_to_file(&id).await {
                Ok(bundle) => NodeResponse::Exported { bundle },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::Import { bundle } => match self.runtime.import(bundle).await {
                Ok(_) => NodeResponse::Imported,
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::Shutdown => NodeResponse::ShutdownAccepted,

            NodeCommand::Dial { addr } => match addr.parse() {
                Ok(addr) => match self.runtime.network.dial(addr).await {
                    Ok(_) => NodeResponse::Dialed,
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                },
                Err(e) => NodeResponse::Error {
                    message: format!("Invalid address: {e}"),
                },
            },

            NodeCommand::ListenViaRelay { relay_addr } => match relay_addr.parse() {
                Ok(relay_addr) => match self.runtime.network.listen_via_relay(relay_addr).await {
                    Ok(_) => NodeResponse::ListeningViaRelay,
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                },
                Err(e) => NodeResponse::Error {
                    message: format!("Invalid address: {e}"),
                },
            },

            NodeCommand::Publish { topic, data } => {
                match self.runtime.network.publish(&topic, data).await {
                    Ok(_) => NodeResponse::Published,
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            // Intercepted in `handle_connection` before reaching here.
            NodeCommand::Subscribe { .. } => NodeResponse::Error {
                message: "Subscribe must be handled as a streaming connection".to_string(),
            },

            NodeCommand::Peers => match self.runtime.network.peers().await {
                Ok(peers) => NodeResponse::Peers {
                    peers: peers
                        .into_iter()
                        .map(|peer| canopee_protocol::PeerInfo {
                            peer_id: peer.peer_id.to_string(),
                            identity: peer.identity,
                            addresses: peer.addresses.iter().map(|a| a.to_string()).collect(),
                        })
                        .collect(),
                },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::RelayReservations => match self.runtime.network.relay_reservations().await
            {
                Ok(reservations) => NodeResponse::RelayReservations {
                    reservations: reservations
                        .into_iter()
                        .map(|r| canopee_protocol::RelayReservationInfo {
                            relay_peer_id: r.relay_peer_id.to_string(),
                            renewal: r.renewal,
                            listen_addrs: r.listen_addrs.iter().map(|a| a.to_string()).collect(),
                        })
                        .collect(),
                },
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },

            NodeCommand::FindProviders { id } => {
                match self.runtime.network.find_providers(id).await {
                    Ok(peer_ids) => NodeResponse::Providers {
                        peer_ids: peer_ids.iter().map(|p| p.to_string()).collect(),
                    },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }

            NodeCommand::FetchObject { peer_id, id } => match peer_id.parse() {
                Ok(peer_id) => match self.runtime.network.get_object(peer_id, id).await {
                    Ok(bundle) => NodeResponse::Exported { bundle },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                },
                Err(e) => NodeResponse::Error {
                    message: format!("Invalid peer id: {e}"),
                },
            },

            NodeCommand::Announce { id } => match self.runtime.network.announce(id).await {
                Ok(_) => NodeResponse::Announced,
                Err(e) => NodeResponse::Error {
                    message: e.to_string(),
                },
            },
            NodeCommand::PutObject { data, object_type } => {
                match self.runtime.put_object(data, object_type).await {
                    Ok(object) => NodeResponse::ObjectCreated { id: object.id },
                    Err(e) => NodeResponse::Error {
                        message: e.to_string(),
                    },
                }
            }
        }
    }
}

#[tokio::test]
async fn test_node_put() {
    let runtime = Runtime::open().await.unwrap();
    let node = Node::new(runtime);
    let response = node
        .handle(NodeCommand::Put {
            data: b"hello".to_vec(),
        })
        .await;

    println!("{:?}", response);
}
