use canopee_protocol::{NodeCommand, NodeResponse};
use canopee_runtime::Runtime;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

#[derive(Clone)]
pub struct Node {
    runtime: Arc<Runtime>,
}

impl Node {
    pub fn new(runtime: Runtime) -> Self {
        Self {
            runtime: Arc::new(runtime),
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

        loop {
            let (stream, _) = listener.accept().await?;
            let node = self.clone();
            tokio::spawn(async move {
                if let Err(e) = node.handle_connection(stream).await {
                    eprintln!("Connection error: {}", e);
                }
            });
        }
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

        let response = self.handle(command).await;

        self.write_response(&mut stream, response).await?;

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
                NodeResponse::Status {
                    identity: self.runtime.identity().id().to_string(),
                    objects,
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
