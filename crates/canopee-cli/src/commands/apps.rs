use std::path::Path;

use canopee_protocol::{NodeCommand, NodeResponse};
use canopee_sdk::{CanopeeClient, NodeClient};
use canopee_storage::{AppManifest, ObjectId, ObjectType};

use crate::app::{fetch_app, publish_directory, serve};
use crate::uri;

use super::common::{open_with_default_app, resolve_object_arg, serve_app_on_http};

pub(crate) async fn app_manifest(directory_path: String, name: String) {
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
        .put_object(bytes, ObjectType::AppManifest, Some(manifest.name.clone()))
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

pub(crate) async fn app_info(id: String) {
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

pub(crate) async fn open(
    id: Option<String>,
    owner: Option<String>,
    name: Option<String>,
    peer: Option<String>,
    port: u16,
    open: bool,
) {
    let client = CanopeeClient::connect().await.unwrap();

    match (id, owner, name) {
        (Some(arg), _, _) => {
            let (object_id, display_name) = match resolve_object_arg(&client, &arg).await {
                Ok(resolved) => resolved,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            };
            // A locally-stored file opens with the OS default app;
            // anything else (a manifest, or a remote fetch) keeps the
            // app-serving behavior.
            match client.get(object_id.clone()).await {
                Ok(object) if object.payload.object_type != ObjectType::AppManifest => {
                    open_with_default_app(&object.payload.data, &display_name).await;
                }
                _ => serve_app_on_http(&client, object_id, peer, port, open).await,
            }
        }
        (None, Some(owner), Some(name)) => {
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
            serve_app_on_http(&client, manifest_id, peer, port, open).await;
        }
        _ => {
            eprintln!("Error: pass either <id> or both --owner and --name");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn handle(uri: String, port: u16) {
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