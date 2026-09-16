use canopee_sdk::{CanopeeClient, ObjectId};

/// Example of an app using canopee-sdk against a running node.
///
/// Usage:
///   cargo run -p canopee-sdk --example app -- identity
///   cargo run -p canopee-sdk --example app -- put <text>
///   cargo run -p canopee-sdk --example app -- get <object-id>
///   cargo run -p canopee-sdk --example app -- peers
///   cargo run -p canopee-sdk --example app -- publish <topic> <text>
///   cargo run -p canopee-sdk --example app -- subscribe <topic>
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let client = CanopeeClient::connect().await?;

    match args.first().map(String::as_str) {
        Some("identity") => {
            println!("{}", client.identity().await?);
        }
        Some("put") => {
            let text = args.get(1).expect("usage: put <text>");
            let id = client.put(text.as_bytes().to_vec(), None).await?;
            println!("{id}");
        }
        Some("get") => {
            let id = ObjectId::new(args.get(1).expect("usage: get <object-id>"));
            let object = client.get(id).await?;
            println!("{}", String::from_utf8_lossy(&object.payload.data));
        }
        Some("peers") => {
            for peer in client.peers().await? {
                println!("{}", peer.peer_id);
            }
        }
        Some("announce") => {
            let id = ObjectId::new(args.get(1).expect("usage: announce <object-id>"));
            client.announce(id).await?;
            println!("announced");
        }
        Some("find-providers") => {
            let id = ObjectId::new(args.get(1).expect("usage: find-providers <object-id>"));
            for peer_id in client.find_providers(id).await? {
                println!("{peer_id}");
            }
        }
        Some("fetch") => {
            let peer_id = args.get(1).expect("usage: fetch <peer-id> <object-id>");
            let id = ObjectId::new(args.get(2).expect("usage: fetch <peer-id> <object-id>"));
            let bundle = client.fetch_object(peer_id.clone(), id).await?;
            println!("{}", String::from_utf8_lossy(&bundle.object.payload.data));
        }
        Some("publish") => {
            let topic = args.get(1).expect("usage: publish <topic> <text>");
            let text = args.get(2).expect("usage: publish <topic> <text>");
            client.publish(topic, text.as_bytes().to_vec()).await?;
            println!("published");
        }
        Some("subscribe") => {
            let topic = args.get(1).expect("usage: subscribe <topic>");
            let mut subscription = client.subscribe(topic).await?;
            println!("subscribed to {topic}, waiting for messages...");
            while let Some(message) = subscription.next().await? {
                println!(
                    "[{}] {}",
                    message.topic,
                    String::from_utf8_lossy(&message.data)
                );
            }
        }
        _ => {
            eprintln!("unknown or missing command");
        }
    }

    Ok(())
}
