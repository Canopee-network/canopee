use std::env;
use std::path::Path;

use canopee_sdk::CanopeeClient;
use canopee_storage::{AppManifest, ObjectType};

use crate::app::publish_directory;

fn app_name_from_directory(directory: &str) -> String {
    Path::new(directory)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app")
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Publishes a local static website to the internet.
///
/// Uploads every file in `<directory>` as an immutable object, writes an app
/// manifest (whose id is a hash of the owner identity + content), then
/// starts a foreground serve session that tunnels requests through a Canopee
/// edge at `https://<app-hash>.<domain>/`. The app id *is* the address — no
/// username needed. Press Ctrl+C to stop serving.
pub(crate) async fn publish(directory: String) {
    let client = match CanopeeClient::connect().await {
        Ok(client) => client,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // 1. Pick the edge to publish through. Default: the built-in bootstrap
    //    relay, which runs the edge role like every other node. Override with
    //    CANOPEE_EDGE_ADDR to publish through a different edge (e.g. your own
    //    domain's).
    let edge_addr = match env::var("CANOPEE_EDGE_ADDR") {
        Ok(addr) if !addr.trim().is_empty() => addr,
        _ => canopee_network::DEFAULT_BOOTSTRAP_ADDRS[0].to_string(),
    };
    let base_domain =
        env::var("CANOPEE_PUBLIC_BASE_DOMAIN").unwrap_or_else(|_| "canopee.network".to_string());

    // 2. Upload the files (suppressing per-file progress), build the manifest,
    //    and point `app:<dirname>` at it so `open --name <dirname>` still
    //    resolves to the same app locally.
    let (entrypoint, assets) = match publish_directory(&client, Path::new(&directory), true).await {
        Ok(published) => published,
        Err(e) => {
            eprintln!("Error: failed to publish directory: {e:#}");
            std::process::exit(1);
        }
    };
    let identity = match client.identity().await {
        Ok(identity) => identity,
        Err(e) => {
            eprintln!("Error: failed to read identity: {e}");
            std::process::exit(1);
        }
    };
    let app_name = app_name_from_directory(&directory);
    let manifest = AppManifest {
        name: app_name.clone(),
        owner: identity,
        entrypoint,
        assets,
    };
    let bytes = match bincode::serialize(&manifest) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("Error: failed to serialize app manifest: {e}");
            std::process::exit(1);
        }
    };
    let manifest_id = match client
        .put_object(bytes, ObjectType::AppManifest, Some(app_name.clone()))
        .await
    {
        Ok(object_id) => object_id,
        Err(e) => {
            eprintln!("Error: failed to store app manifest: {e}");
            std::process::exit(1);
        }
    };
    for object_id in std::iter::once(manifest_id.clone())
        .chain(std::iter::once(manifest.entrypoint.clone()))
        .chain(manifest.assets.values().cloned())
    {
        if let Err(e) = client.announce(object_id).await {
            eprintln!("Warning: failed to announce object: {e}");
        }
    }
    // Best-effort: the `app:<name>` pointer only powers local
    // `open --name <dirname>` lookups. The serve session and the edge resolve
    // the app by manifest id, so a DHT hiccup here must not fail the publish.
    let pointer = format!("app:{app_name}");
    if let Err(e) = client.publish_pointer(pointer, manifest_id.clone()).await {
        eprintln!("Warning: failed to publish app pointer (open --name will lag): {e}");
    }

    // 3. Start the serve session and keep it in the foreground. The edge
    //    verifies our ownership by fetching the manifest and checking its
    //    owner — the app id is a hash of it, so only we can register it.
    //    Ctrl+C sends a signed deregistration (timestamp 0) and stops.
    match client.start_serve_session(&edge_addr, &manifest_id).await {
        Ok((_, _root_url)) => {}
        Err(e) => {
            eprintln!("Error: failed to start serve session: {e:#}");
            std::process::exit(1);
        }
    }

    let url = format!(
        "https://{}.{base_domain}/",
        canopee_network::app_subdomain(&manifest_id.to_string())
    );

    println!("Publishing \"{directory}\" as {app_name}");
    println!("  Manifest: {manifest_id}");
    println!("  \u{2713} Application published!");
    println!("  Public: {url}");
    println!();
    println!("  Serving live from this node. Press Ctrl+C to take it offline.");

    if let Err(e) = tokio::signal::ctrl_c().await {
        eprintln!("Error: failed to listen for Ctrl+C: {e}");
        std::process::exit(1);
    }

    match client.stop_serve_session().await {
        Ok(()) => println!("Application taken offline."),
        Err(e) => eprintln!("Warning: failed to stop serve session: {e}"),
    }
}
