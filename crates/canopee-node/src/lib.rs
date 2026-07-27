use canopee_identity::IdentityId;
use canopee_runtime::Runtime;
use canopee_storage::{ExportBundle, Object, ObjectId, ObjectInfo};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum NodeCommand {
    Put { data: Vec<u8> },
    Get { id: ObjectId },
    List,
    Export { id: ObjectId },
    Import { bundle: ExportBundle },
    Status,
    Identity,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NodeResponse {
    ObjectCreated { id: ObjectId },
    Object { object: Object },
    Objects { objects: Vec<ObjectInfo> },
    Exported { bundle: ExportBundle },
    Imported,
    Status { identity: String, objects: usize },
    Error { message: String },
    Identity { identity_id: IdentityId },
}

pub struct Node {
    runtime: Runtime,
}

impl Node {
    pub fn new(runtime: Runtime) -> Self {
        Self { runtime }
    }

    pub async fn open() -> anyhow::Result<Self> {
        let runtime = Runtime::open().await?;

        Ok(Self::new(runtime))
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        self.runtime.run().await
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
