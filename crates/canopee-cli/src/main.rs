use canopee_client::NodeClient;
// use canopee_node::Node;
use canopee_protocol::{NodeCommand, NodeResponse};
use canopee_runtime::Runtime;
use canopee_storage::{ExportBundle, ObjectId};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "canopee")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Init,
    Identity,
    List,
    Start,
    Get { id: String },
    Put { path: String },
    Export { id: String },
    Import { path: String },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => {
            let runtime = Runtime::open().await.unwrap();
            println!("Canopee initialized:");
            println!("Identity: {:?}", runtime.identity().id());
        }

        Commands::Identity => {
            let client = NodeClient::new().await.unwrap();
            let response = client.request(NodeCommand::Identity).await.unwrap();

            match response {
                NodeResponse::Identity { identity_id } => {
                    println!("{:?}", identity_id);
                }

                NodeResponse::Error { message } => {
                    eprintln!("{}", message);
                }
                _ => {}
            }
        }

        Commands::Put { path } => {
            let data = tokio::fs::read(path).await.unwrap();
            let client = NodeClient::new().await.unwrap();
            let response = client.request(NodeCommand::Put { data }).await.unwrap();

            match response {
                NodeResponse::ObjectCreated { id } => {
                    println!("Created object:");
                    println!("{}", id);
                }

                NodeResponse::Error { message } => {
                    eprintln!("Error: {}", message);
                }
                _ => {}
            }
        }

        Commands::Get { id } => {
            let object_id = ObjectId::new(&id);
            let client = NodeClient::new().await.unwrap();
            let response = client
                .request(NodeCommand::Get { id: object_id })
                .await
                .unwrap();

            match response {
                NodeResponse::Object { object } => {
                    println!("Object:");
                    println!("{:?}", object.id);
                }

                NodeResponse::Error { message } => {
                    eprintln!("Error: {}", message);
                }
                _ => {}
            }
        }

        Commands::List => {
            let client = NodeClient::new().await.unwrap();
            let response = client.request(NodeCommand::List).await.unwrap();

            match response {
                NodeResponse::Objects { objects } => {
                    println!("Canopee Objects:\n");
                    for object in objects {
                        println!("{}", object.id);
                        println!("Owner: {:?}", object.owner);
                        println!("Size: {} bytes", object.size);
                        println!(
                            "Signature: {}",
                            if object.verified {
                                "✓ valid"
                            } else {
                                "✗ invalid"
                            }
                        );
                        println!();
                    }
                }

                NodeResponse::Error { message } => {
                    eprintln!("Error: {}", message);
                }
                _ => {}
            }
        }

        Commands::Export { id } => {
            let object_id = ObjectId::new(&id);
            let client = NodeClient::new().await.unwrap();
            let response = client
                .request(NodeCommand::Export { id: object_id })
                .await
                .unwrap();

            match response {
                NodeResponse::Exported { bundle } => {
                    println!("Exported: {:?}", bundle.object.id);
                }

                NodeResponse::Error { message } => {
                    eprintln!("Error: {}", message);
                }
                _ => {}
            }
        }

        Commands::Import { path } => {
            let bytes = tokio::fs::read(path).await.unwrap();
            let bundle: ExportBundle = bincode::deserialize(&bytes).unwrap();
            let client = NodeClient::new().await.unwrap();
            let response = client
                .request(NodeCommand::Import { bundle })
                .await
                .unwrap();

            match response {
                NodeResponse::Imported => {
                    println!("Imported");
                }
                NodeResponse::Error { message } => {
                    eprintln!("Error: {}", message);
                }
                _ => {}
            }
        }

        Commands::Start => {
            // let child = tokio::process::Command::new("canopee-node")
            //     .spawn()
            //     .unwrap();
            let child = tokio::process::Command::new("cargo")
                .args(["run", "-p", "canopee-node"])
                .spawn()
                .unwrap();

            println!("Canopee node started (pid {})", child.id().unwrap());
            // let node = Node::open().await.unwrap();
            // match node.run().await {
            //     Ok(_) => println!("Node started."),
            //     Err(e) => eprintln!("Error: {}", e),
            // }
        }
    }
}
