use canopee_protocol::{NodeCommand, NodeResponse};
use canopee_runtime::Runtime;
use canopee_sdk::{CanopeeClient, NodeClient};
use canopee_storage::{ExportBundle, ObjectId, ObjectType};
use clap::{Parser, Subcommand};
use tokio::io::{AsyncBufReadExt, BufReader};
mod app;
use app::{AppManifest, publish_directory};
use std::path::Path;

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
    Get {
        id: String,
    },
    Put {
        path: String,
    },
    Export {
        id: String,
    },
    Import {
        path: String,
    },
    Status,
    Stop,
    Dial {
        addr: String,
    },
    ListenViaRelay {
        relay_addr: String,
    },
    Peers,
    RelayStatus,
    Publish {
        topic: String,
        message: String,
    },
    Chat {
        topic: String,
    },
    //canopee-cli app-manifest ./portfolio --name alice-portfolio
    AppManifest {
        directory_path: String,
        name: String,
    },
    AppInfo {
        id: String,
    },
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

        Commands::Peers => {
            let client = NodeClient::new().await.unwrap();
            let response = client.request(NodeCommand::Peers).await.unwrap();

            match response {
                NodeResponse::Peers { peers } => {
                    if peers.is_empty() {
                        println!("No connected peers");
                    }
                    for peer in peers {
                        println!("{}", peer.peer_id);
                        if let Some(identity) = &peer.identity {
                            println!("  Identity: {:?}", identity);
                        }
                        for addr in &peer.addresses {
                            println!("  Address: {}", addr);
                        }
                    }
                }
                NodeResponse::Error { message } => eprintln!("Error: {}", message),
                _ => {}
            }
        }

        Commands::RelayStatus => {
            let client = NodeClient::new().await.unwrap();
            let response = client
                .request(NodeCommand::RelayReservations)
                .await
                .unwrap();

            match response {
                NodeResponse::RelayReservations { reservations } => {
                    if reservations.is_empty() {
                        println!("No accepted relay reservations yet");
                    }
                    for reservation in reservations {
                        println!("Relay: {}", reservation.relay_peer_id);
                        println!("  Renewal: {}", reservation.renewal);
                        if reservation.listen_addrs.is_empty() {
                            println!("  Listen addresses: (pending)");
                        } else {
                            for addr in &reservation.listen_addrs {
                                println!("  Listen address: {}", addr);
                            }
                        }
                    }
                }
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

        Commands::Chat { topic } => {
            let client = CanopeeClient::connect().await.unwrap();
            let mut subscription = client.subscribe(topic.clone()).await.unwrap();

            tokio::spawn(async move {
                while let Ok(Some(message)) = subscription.next().await {
                    let text = String::from_utf8_lossy(&message.data);
                    let from = message.source.as_deref().unwrap_or("unknown");
                    println!("{from}: {text}");
                }
                println!("Subscription closed");
            });

            println!("Chatting on topic '{topic}'. Type a message and press enter to send.");
            let mut lines = BufReader::new(tokio::io::stdin()).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.is_empty() {
                    continue;
                }
                if let Err(e) = client.publish(topic.clone(), line.into_bytes()).await {
                    eprintln!("Error: {}", e);
                }
            }
        }

        Commands::Start => {
            let child = tokio::process::Command::new("cargo")
                .args(["run", "-p", "canopee-node"])
                .spawn()
                .unwrap();

            println!("Canopee node started (pid {})", child.id().unwrap());
        }

        Commands::AppManifest {
            directory_path,
            name,
        } => {
            let client = CanopeeClient::connect().await.unwrap();
            let (entrypoint, assets) = publish_directory(&client, Path::new(&directory_path))
                .await
                .unwrap();
            let identity = client.identity().await.unwrap();
            let manifest = AppManifest {
                name,
                owner: identity,
                entrypoint,
                assets,
            };
            let bytes = bincode::serialize(&manifest).unwrap();
            let object_id = client
                .put_object(bytes, ObjectType::AppManifest)
                .await
                .unwrap();

            println!();
            println!("Application published:");
            println!("{}", object_id);
        }

        Commands::AppInfo { id } => {
            let object_id = ObjectId::new(&id);
            let client = NodeClient::new().await.unwrap();
            let response = client
                .request(NodeCommand::Get { id: object_id })
                .await
                .unwrap();

            match response {
                NodeResponse::Object { object } => {
                    let manifest: AppManifest = object.decode().unwrap();

                    println!("Manifest:");
                    println!("{:#?}", manifest);
                }

                NodeResponse::Error { message } => {
                    eprintln!("Error: {}", message);
                }
                _ => {}
            }
        }
    }
}
