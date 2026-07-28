use canopee_protocol::{NodeCommand, NodeResponse};
use canopee_runtime::Runtime;
use canopee_sdk::NodeClient;
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
    Status,
    Stop,
    Dial { addr: String },
    ListenViaRelay { relay_addr: String },
    Publish { topic: String, message: String },
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

        Commands::Status => {
            let client = NodeClient::new().await.unwrap();
            let response = client.request(NodeCommand::Status).await.unwrap();
            match response {
                NodeResponse::Status {
                    identity,
                    objects,
                    peers,
                } => {
                    println!("\nCanopee Node");
                    println!("\nRunning:");
                    println!("yes"); // placeholder
                    println!("\nIdentity:");
                    println!("{}", identity);
                    println!("\nObjects:");
                    println!("{}", objects);
                    println!("\nPeers:");
                    println!("{}", peers);
                }
                NodeResponse::Error { message } => {
                    eprintln!("Error: {}", message);
                }
                _ => {}
            }
        }

        Commands::Stop => {
            let client = NodeClient::new().await.unwrap();
            let response = client.request(NodeCommand::Shutdown).await.unwrap();

            match response {
                NodeResponse::ShutdownAccepted => {
                    println!("Canopee node stopped");
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

        Commands::Dial { addr } => {
            let client = NodeClient::new().await.unwrap();
            let response = client.request(NodeCommand::Dial { addr }).await.unwrap();

            match response {
                NodeResponse::Dialed => println!("Dialing..."),
                NodeResponse::Error { message } => eprintln!("Error: {}", message),
                _ => {}
            }
        }

        Commands::ListenViaRelay { relay_addr } => {
            let client = NodeClient::new().await.unwrap();
            let response = client
                .request(NodeCommand::ListenViaRelay { relay_addr })
                .await
                .unwrap();

            match response {
                NodeResponse::ListeningViaRelay => println!("Requesting relay reservation..."),
                NodeResponse::Error { message } => eprintln!("Error: {}", message),
                _ => {}
            }
        }

        Commands::Publish { topic, message } => {
            let client = NodeClient::new().await.unwrap();
            let response = client
                .request(NodeCommand::Publish {
                    topic,
                    data: message.into_bytes(),
                })
                .await
                .unwrap();

            match response {
                NodeResponse::Published => println!("Published"),
                NodeResponse::Error { message } => eprintln!("Error: {}", message),
                _ => {}
            }
        }

        Commands::Start => {
            let child = tokio::process::Command::new("cargo")
                .args(["run", "-p", "canopee-node"])
                .spawn()
                .unwrap();

            println!("Canopee node started (pid {})", child.id().unwrap());
        }
    }
}
