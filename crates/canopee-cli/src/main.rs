use canopee_protocol::{NodeCommand, NodeResponse};
use canopee_runtime::Runtime;
use canopee_sdk::{CanopeeClient, NodeClient};
use canopee_storage::{AppManifest, ExportBundle, ObjectId, ObjectType};
use clap::{Parser, Subcommand};
use tokio::io::{AsyncBufReadExt, BufReader};
mod app;
mod uri;
use app::{fetch_app, publish_directory, serve};
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
    /// Announces on the DHT that this node provides the given object, so
    /// other peers can discover it via `find-providers`.
    Announce {
        id: String,
    },
    /// Lists peer ids that have announced themselves as providers of the
    /// given object on the DHT.
    FindProviders {
        id: String,
    },
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
    /// Fetches an app manifest and its assets (from a peer if not stored
    /// locally) and serves them over HTTP for viewing in a browser. Either
    /// pass a manifest id directly, or `--owner`/`--name` to resolve the
    /// latest manifest published under that name (so republishing doesn't
    /// require sharing a new id).
    Open {
        id: Option<String>,
        #[arg(long)]
        owner: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        peer: Option<String>,
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Open the served URL in the default browser instead of just printing it.
        #[arg(long)]
        open: bool,
    },
    /// Resolves a `canopee://` URI (e.g. `canopee://alice/portfolio`) to an
    /// app and serves it in the default browser. Used by the OS-level URI
    /// scheme handler registered via `canopee uri-register`.
    Handle {
        uri: String,
        #[arg(long, default_value_t = 0)]
        port: u16,
    },
    /// Manages friendly short names (e.g. `alice`) that resolve to canonical
    /// `canopee://identity/<peer-id>` owners inside `canopee://` URIs.
    Alias {
        #[command(subcommand)]
        command: AliasCommand,
    },
    /// Registers this machine's OS to route `canopee://` URIs to `canopee handle`.
    UriRegister,
    /// Removes the OS-level `canopee://` scheme registration.
    UriUnregister,
}

#[derive(Subcommand)]
enum AliasCommand {
    /// Maps `<name>` to a canonical owner (`canopee://identity/<peer-id>`).
    Set {
        name: String,
        owner: String,
    },
    /// Lists all known aliases.
    List,
    /// Removes `<name>`.
    #[command(alias = "rm")]
    Remove {
        name: String,
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

        Commands::Announce { id } => {
            let client = NodeClient::new().await.unwrap();
            let object_id = ObjectId::new(&id);
            let response = client
                .request(NodeCommand::Announce { id: object_id })
                .await
                .unwrap();

            match response {
                NodeResponse::Announced => println!("Announced {}", id),
                NodeResponse::Error { message } => eprintln!("Error: {}", message),
                _ => {}
            }
        }

        Commands::FindProviders { id } => {
            let object_id = ObjectId::new(&id);
            let client = NodeClient::new().await.unwrap();
            let response = client
                .request(NodeCommand::FindProviders { id: object_id })
                .await
                .unwrap();

            match response {
                NodeResponse::Providers { peer_ids } => {
                    if peer_ids.is_empty() {
                        println!("No providers found for {id}");
                    }
                    for peer_id in peer_ids {
                        println!("{}", peer_id);
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

            client.announce(object_id.clone()).await.unwrap();
            client.announce(manifest.entrypoint.clone()).await.unwrap();
            for asset_id in manifest.assets.values() {
                client.announce(asset_id.clone()).await.unwrap();
            }
            client
                .publish_app_pointer(manifest.name.clone(), object_id.clone())
                .await
                .unwrap();

            println!();
            println!("Application published and announced:");
            println!("{}", object_id);
            println!();
            println!(
                "Republishing under the same name (\"{}\") will update what `open --owner ... --name {}` resolves to.",
                manifest.name, manifest.name
            );
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

        Commands::Open {
            id,
            owner,
            name,
            peer,
            port,
            open,
        } => {
            let client = CanopeeClient::connect().await.unwrap();

            let manifest_id = match (id, owner, name) {
                (Some(id), _, _) => ObjectId::new(&id),
                (None, Some(owner), Some(name)) => client
                    .resolve_app_pointer(canopee_sdk::IdentityId::new(owner), name.clone())
                    .await
                    .unwrap()
                    .unwrap_or_else(|| panic!("no app pointer found for \"{name}\"")),
                _ => panic!("pass either <id> or both --owner and --name"),
            };

            let (manifest, files) = match fetch_app(&client, manifest_id, peer).await {
                Ok(app) => app,
                Err(e) => {
                    eprintln!("Error: failed to open app: {e:#}");
                    std::process::exit(1);
                }
            };

            println!("Opening \"{}\" by {}", manifest.name, manifest.owner);
            serve(files, port, open).await.unwrap();
        }

        Commands::Handle { uri, port } => {
            let (owner, name) = match uri::split_uri(&uri) {
                Ok(parts) => parts,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            };
            let owner = match uri::resolve_owner(&owner) {
                Ok(owner) => owner,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            };

            let client = match CanopeeClient::connect().await {
                Ok(client) => client,
                Err(e) => {
                    // Remember the URI so it can be retried once a node is up,
                    // rather than silently dropping the user's click.
                    let pending = uri::pending_uri_path();
                    let _ = std::fs::create_dir_all(pending.parent().unwrap_or(Path::new(".")));
                    let _ = std::fs::write(&pending, &uri);
                    eprintln!("Error: {e}");
                    eprintln!(
                        "The URI was saved to {} — start a node (`canopee start`) and re-run: canopee handle \"{uri}\"",
                        pending.display()
                    );
                    std::process::exit(1);
                }
            };

            let manifest_id = match client
                .resolve_app_pointer(canopee_sdk::IdentityId::new(owner), name.clone())
                .await
            {
                Ok(Some(id)) => id,
                Ok(None) => {
                    eprintln!("Error: no app pointer found for \"{name}\"");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Error: failed to resolve app pointer: {e:#}");
                    std::process::exit(1);
                }
            };

            let (manifest, files) = match fetch_app(&client, manifest_id, None).await {
                Ok(app) => app,
                Err(e) => {
                    eprintln!("Error: failed to open app: {e:#}");
                    std::process::exit(1);
                }
            };

            println!("Opening \"{}\" by {}", manifest.name, manifest.owner);
            serve(files, port, true).await.unwrap();
        }

        Commands::Alias { command } => match command {
            AliasCommand::Set { name, owner } => match uri::set_alias(&name, &owner) {
                Ok(()) => println!("alias \"{name}\" -> {owner}"),
                Err(e) => eprintln!("Error: {e:#}"),
            },
            AliasCommand::List => match uri::list_aliases() {
                Ok(aliases) => {
                    if aliases.is_empty() {
                        println!("No aliases set");
                    }
                    for (name, owner) in aliases {
                        println!("{name} -> {owner}");
                    }
                }
                Err(e) => eprintln!("Error: {e:#}"),
            },
            AliasCommand::Remove { name } => match uri::remove_alias(&name) {
                Ok(true) => println!("Removed alias \"{name}\""),
                Ok(false) => println!("No alias \"{name}\" found"),
                Err(e) => eprintln!("Error: {e:#}"),
            },
        },

        Commands::UriRegister => match uri::register_scheme() {
            Ok(()) => println!("Registered canopee:// scheme handler"),
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        },

        Commands::UriUnregister => match uri::unregister_scheme() {
            Ok(()) => println!("Removed canopee:// scheme handler"),
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        },
    }
}
