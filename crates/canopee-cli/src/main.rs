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
    /// Fetches an object from a specific peer and imports it locally.
    Fetch {
        /// The peer id, identity (`canopee://identity/...`), or username to
        /// fetch from.
        peer_id: String,
        id: String,
    },
    /// Shares a stored object under a name: upserts a `shared: true` entry in
    /// your home index and announces the object on the DHT, so any peer can
    /// discover and fetch it.
    Share {
        name: String,
        id: String,
    },
    /// Stops sharing the home entry `<name>`: the object is withdrawn from
    /// the DHT and no longer served to peers.
    Unshare {
        name: String,
    },
    /// Lists the entries in your home index (name, object id, shared flag).
    Home,
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
    /// Claims and looks up globally unique usernames. A claimed username lets
    /// other peers discover and address you by name instead of a raw peer id
    /// (e.g. `canopee fetch <name> <id>`).
    Username {
        #[command(subcommand)]
        command: UsernameCommand,
    },
}

#[derive(Subcommand)]
enum UsernameCommand {
    /// Claims `<username>` for this node's identity so peers can discover it
    /// by name.
    Claim {
        username: String,
    },
    /// Shows the username currently claimed by this node's identity.
    Show,
    /// Reverse-resolves `<username>` to its canonical owner identity.
    Lookup {
        username: String,
    },
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
            let client = CanopeeClient::connect().await.unwrap();
            match client.peers().await {
                Ok(peers) => {
                    if peers.is_empty() {
                        println!("No connected peers");
                    }
                    for peer in peers {
                        let name = peer
                            .display_name
                            .clone()
                            .or(peer.username.clone())
                            .unwrap_or_else(|| short_peer_id(&peer.peer_id));
                        println!("{name}");
                        println!("  Peer ID: {}", peer.peer_id);
                        if let Some(username) = &peer.username {
                            println!("  Username: {username}");
                        }
                        if let Some(identity) = &peer.identity {
                            println!("  Identity: {identity}");
                        }
                        for addr in &peer.addresses {
                            println!("  Address: {addr}");
                        }
                    }
                }
                Err(e) => eprintln!("Error: {e}"),
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

        Commands::Fetch { peer_id, id } => {
            let client = CanopeeClient::connect().await.unwrap();
            match resolve_peer_arg(&client, &peer_id).await {
                Ok(peer_id) => {
                    match client.fetch_object(peer_id, ObjectId::new(&id)).await {
                        Ok(bundle) => {
                            // Import so the object is stored (and re-served as cache).
                            client.import(bundle).await.unwrap();
                            println!("Fetched and imported {}", id);
                        }
                        Err(e) => {
                            eprintln!("Error: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Share { name, id } => {
            let client = CanopeeClient::connect().await.unwrap();
            match client
                .share_object(name.clone(), ObjectId::new(&id), Some("cli".into()))
                .await
            {
                Ok(_) => println!("Shared \"{}\" ({})", name, id),
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Unshare { name } => {
            let client = CanopeeClient::connect().await.unwrap();
            match client.unshare(name.clone()).await {
                Ok(_) => println!("Unshared \"{}\"", name),
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Home => {
            let client = CanopeeClient::connect().await.unwrap();
            match client.load_home_index().await {
                Ok(Some(index)) => {
                    if index.entries.is_empty() {
                        println!("Home index is empty");
                    }
                    for entry in index.entries {
                        println!(
                            "{}\n  Object: {}\n  Type: {:?}\n  Shared: {}\n  App: {}",
                            entry.name,
                            entry.object,
                            entry.object_type,
                            if entry.shared { "yes" } else { "no" },
                            entry.app.as_deref().unwrap_or("-"),
                        );
                    }
                }
                Ok(None) => println!("No home index yet"),
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        Commands::Chat { topic } => {
            let client = CanopeeClient::connect().await.unwrap();
            // Resolve connected peers' friendly names up front so incoming
            // messages display as names rather than raw peer ids. Best-effort:
            // unknown senders fall back to a shortened peer id.
            let mut names: std::collections::HashMap<String, String> = client
                .peers()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|p| {
                    let name = p
                        .display_name
                        .clone()
                        .or(p.username.clone())
                        .unwrap_or_else(|| short_peer_id(&p.peer_id));
                    (p.peer_id, name)
                })
                .collect();
            let mut subscription = client.subscribe(topic.clone()).await.unwrap();

            tokio::spawn(async move {
                while let Ok(Some(message)) = subscription.next().await {
                    let text = String::from_utf8_lossy(&message.data);
                    let from = message.source.as_deref().unwrap_or("unknown");
                    let name = names
                        .entry(from.to_string())
                        .or_insert_with(|| short_peer_id(from));
                    println!("{name}: {text}");
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

        Commands::Username { command } => {
            let client = CanopeeClient::connect().await.unwrap();
            match command {
                UsernameCommand::Claim { username } => {
                    match client.claim_username(&username).await {
                        Ok(()) => println!("Claimed username \"{username}\""),
                        Err(e) => {
                            eprintln!("Error: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                UsernameCommand::Show => match client.show_username().await {
                    Ok(Some(username)) => println!("{username}"),
                    Ok(None) => {
                        println!("No username claimed yet (claim one with `canopee username claim <name>`)")
                    }
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                },
                UsernameCommand::Lookup { username } => {
                    match client.resolve_username(&username).await {
                        Ok(Some(owner)) => println!("{username} -> {owner}"),
                        Ok(None) => println!("No username \"{username}\" claimed"),
                        Err(e) => {
                            eprintln!("Error: {e}");
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
    }
}

/// Shortens a raw libp2p peer id for display (`12D3KooW…abcd`) so terminal
/// output isn't dominated by a 39-character base58 blob.
fn short_peer_id(peer_id: &str) -> String {
    const HEAD: usize = 12;
    const TAIL: usize = 4;
    if peer_id.len() <= HEAD + TAIL + 1 {
        return peer_id.to_string();
    }
    format!("{}…{}", &peer_id[..HEAD], &peer_id[peer_id.len() - TAIL..])
}

/// Resolves a CLI peer argument to a dialable peer id string. Accepts:
/// - a raw libp2p peer id (passed through),
/// - a canonical identity `canopee://identity/<peer-id>` (peer id extracted),
/// - a friendly username, reverse-resolved via the DHT registry.
async fn resolve_peer_arg(client: &CanopeeClient, arg: &str) -> anyhow::Result<String> {
    let arg = arg.trim();
    // Canonical identity form: extract the embedded peer id.
    if let Some(peer_id) = arg.strip_prefix("canopee://identity/") {
        return Ok(peer_id.to_string());
    }
    // Raw peer id: libp2p Ed25519 peer ids always start with this prefix.
    if arg.starts_with("12D3KooW") {
        return Ok(arg.to_string());
    }
    // Otherwise treat it as a username and reverse-resolve it.
    match client.resolve_username(arg).await? {
        Some(owner) => {
            let owner = owner.to_string();
            owner
                .strip_prefix("canopee://identity/")
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow::anyhow!("malformed owner identity: {owner}"))
        }
        None => anyhow::bail!(
            "\"{arg}\" is not a valid peer id, and no username \"{arg}\" is claimed \
             (check with `canopee username lookup {arg}`)"
        ),
    }
}
