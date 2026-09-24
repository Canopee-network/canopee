use canopee_sdk::CanopeeClient;

use super::common::resolve_object_arg;

pub(crate) async fn share(name: String, id: Option<String>, ids: bool) {
    let client = CanopeeClient::connect().await.unwrap();
    // The object is the explicit `<id>` argument if given, otherwise
    // the local object already recorded under the share name.
    let object_arg = id.clone().unwrap_or_else(|| name.clone());
    let (object_id, _) = match resolve_object_arg(&client, &object_arg).await {
        Ok(resolved) => resolved,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };
    client
        .share_object(name.clone(), object_id.clone(), Some("cli".into()))
        .await
        .unwrap();
    if ids {
        println!("Shared \"{name}\"");
        println!("Id: {object_id}");
    } else {
        println!("Shared \"{name}\"");
    }
}

pub(crate) async fn unshare(name: String) {
    let client = CanopeeClient::connect().await.unwrap();
    match client.unshare(name.clone()).await {
        Ok(_) => println!("Unshared \"{}\"", name),
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

pub(crate) async fn home(ids: bool) {
    let client = CanopeeClient::connect().await.unwrap();
    match client.load_home_index().await {
        Ok(Some(index)) => {
            if index.entries.is_empty() {
                println!("Home index is empty");
            }
            for entry in index.entries {
                if ids {
                    println!(
                        "{}\n  Object: {}\n  Type: {:?}\n  Shared: {}\n  App: {}",
                        entry.name,
                        entry.object,
                        entry.object_type,
                        if entry.shared { "yes" } else { "no" },
                        entry.app.as_deref().unwrap_or("-"),
                    );
                } else {
                    println!(
                        "{}\n  Type: {:?}\n  Shared: {}\n  App: {}",
                        entry.name,
                        entry.object_type,
                        if entry.shared { "yes" } else { "no" },
                        entry.app.as_deref().unwrap_or("-"),
                    );
                }
            }
        }
        Ok(None) => println!("No home index yet"),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub(crate) async fn sync(peer_id: Option<String>) {
    let client = CanopeeClient::connect().await.unwrap();
    let result = match peer_id {
        Some(peer_id) => client.sync_with_peer(peer_id).await,
        None => client.sync_with_all_devices().await,
    };
    match result {
        Ok(result) => {
            if result.any_updated() {
                println!("Synced:");
                if result.profile_updated {
                    println!("  profile updated");
                }
                if result.contacts_updated {
                    println!("  contacts updated");
                }
                if result.devices_updated {
                    println!("  devices updated");
                }
            } else {
                println!("Already up to date");
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}
