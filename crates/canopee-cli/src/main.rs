use canopee_node::{Node, NodeCommand, NodeResponse};
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
            let node = Node::open().await.unwrap();
            let response = node.handle(NodeCommand::Identity).await;
            println!("Canopee initialized:");
            println!("{:?}", response);
        }

        Commands::Identity => {
            let node = Node::open().await.unwrap();
            let identity_id = node.handle(NodeCommand::Identity).await;
            println!("Identity ID: {:?}", identity_id);
        }

        Commands::Put { path } => {
            let node = Node::open().await.unwrap();
            let data = tokio::fs::read(path).await.unwrap();

            let response = node.handle(NodeCommand::Put { data }).await;
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
            let node = Node::open().await.unwrap();
            let object_id = ObjectId::new(&id);

            let response = node.handle(NodeCommand::Get { id: object_id }).await;
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
            let node = Node::open().await.unwrap();

            let response = node.handle(NodeCommand::List).await;
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
            let node = Node::open().await.unwrap();
            let object_id = ObjectId::new(&id);

            let response = node.handle(NodeCommand::Export { id: object_id }).await;
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
            let node = Node::open().await.unwrap();
            let bytes = tokio::fs::read(path).await.unwrap();
            let bundle: ExportBundle = bincode::deserialize(&bytes).unwrap();

            let response = node.handle(NodeCommand::Import { bundle }).await;
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
            let node = Node::open().await.unwrap();
            match node.run().await {
                Ok(_) => println!("Node started."),
                Err(e) => eprintln!("Error: {}", e),
            }
        }
    }
}
