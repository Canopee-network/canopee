use canopee_protocol::{NodeCommand, NodeResponse};
use canopee_sdk::{CanopeeClient, NodeClient};
use canopee_storage::ObjectId;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::common::{
    import_idempotent, looks_like_id, resolve_object_arg, resolve_owner_arg, resolve_peer_arg,
    short_peer_id,
};

pub(crate) async fn dial(addr: String) {
    let client = NodeClient::new().await.unwrap();
    let response = client.request(NodeCommand::Dial { addr }).await.unwrap();

    match response {
        NodeResponse::Dialed => println!("Dialing..."),
        NodeResponse::Error { message } => eprintln!("Error: {}", message),
        _ => {}
    }
}

pub(crate) async fn listen_via_relay(relay_addr: String) {
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

pub(crate) async fn peers() {
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

pub(crate) async fn relay_status() {
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

pub(crate) async fn announce(id: String, ids: bool) {
    let client = CanopeeClient::connect().await.unwrap();
    let (object_id, name) = match resolve_object_arg(&client, &id).await {
        Ok(resolved) => resolved,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };
    match client.announce(object_id.clone()).await {
        Ok(()) => {
            if ids {
                println!("Announced {name}");
                println!("Id: {object_id}");
            } else {
                println!("Announced {name}");
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn find_providers(id: String) {
    let client = CanopeeClient::connect().await.unwrap();
    let (object_id, name) = match resolve_object_arg(&client, &id).await {
        Ok(resolved) => resolved,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };
    match client.find_providers(object_id).await {
        Ok(peer_ids) => {
            if peer_ids.is_empty() {
                println!("No providers found for {name}");
            }
            for peer_id in peer_ids {
                println!("{peer_id}");
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn publish(topic: String, message: String) {
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

pub(crate) async fn fetch(peer_id: String, id: String, ids: bool) {
    let client = CanopeeClient::connect().await.unwrap();
    if looks_like_id(&id) {
        match resolve_peer_arg(&client, &peer_id).await {
            Ok(peer_id) => {
                match client.fetch_object(peer_id, ObjectId::new(&id)).await {
                    Ok(bundle) => {
                        // Import so the object is stored (and re-served as cache).
                        import_idempotent(&client, bundle).await;
                        if ids {
                            println!("Fetched {id}");
                        } else {
                            println!("Fetched and imported object");
                        }
                    }
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        // Name-based fetch: resolve the peer's shared entry by name via
        // its `(owner, "entry:<name>")` pointer.
        let (owner, device) = match resolve_owner_arg(&client, &peer_id).await {
            Ok(resolved) => resolved,
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        };
        let record = match client.resolve_pointer(owner, format!("entry:{id}")).await {
            Ok(Some(record)) => record,
            Ok(None) => {
                eprintln!("Error: \"{peer_id}\" has not shared anything named \"{id}\"");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        };
        let target = record.manifest.clone();
        match client.fetch_object(device, target.clone()).await {
            Ok(bundle) => {
                // Import and name it, so the fetched object shows up in
                // `list` and can be `get`/`share`d by name. Import is
                // idempotent: fetching an object you already hold is
                // a no-op, not an error.
                import_idempotent(&client, bundle).await;
                client.set_name(target.clone(), id.clone()).await.unwrap();
                if ids {
                    println!("Fetched \"{id}\" from {peer_id}");
                    println!("Id: {target}");
                } else {
                    println!("Fetched \"{id}\" from {peer_id}");
                }
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    }
}

pub(crate) async fn chat(topic: String) {
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
