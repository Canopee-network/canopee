use canopee_protocol::{NodeCommand, NodeResponse};
use canopee_sdk::{CanopeeClient, NodeClient};
use canopee_storage::{ExportBundle, ObjectId};
use std::path::Path;

use super::common::{hex_preview, object_name, resolve_object_arg};

pub(crate) async fn put(path: String, ids: bool) {
    let data = tokio::fs::read(&path).await.unwrap();
    let name = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    let client = NodeClient::new().await.unwrap();
    let response = client
        .request(NodeCommand::Put {
            data,
            name: name.clone(),
        })
        .await
        .unwrap();

    match response {
        NodeResponse::ObjectCreated { id } => {
            match name {
                Some(name) => {
                    if ids {
                        println!("Stored {name}");
                        println!("Id: {id}");
                    } else {
                        println!("Stored {name}");
                    }
                }
                None => {
                    if ids {
                        println!("Stored object");
                        println!("Id: {id}");
                    } else {
                        println!("Stored object");
                    }
                }
            }
        }

        NodeResponse::Error { message } => {
            eprintln!("Error: {}", message);
        }
        _ => {}
    }
}

pub(crate) async fn get(id: String, output: Option<String>) {
    let client = CanopeeClient::connect().await.unwrap();
    let (object_id, resolved_name) = match resolve_object_arg(&client, &id).await {
        Ok(resolved) => resolved,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };
    match client.get(object_id).await {
        Ok(object) => {
            let name = object_name(&client, &object.id.0).await.unwrap_or(resolved_name);
            match output {
                Some(path) => {
                    tokio::fs::write(&path, &object.payload.data).await.unwrap();
                    println!(
                        "Wrote {} bytes to {path} ({name})",
                        object.payload.data.len()
                    );
                }
                None => {
                    print!("{}", String::from_utf8_lossy(&object.payload.data));
                    if !object.payload.data.is_empty() && !object.payload.data.ends_with(b"\n") {
                        println!();
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn desc(id: String, ids: bool) {
    let client = CanopeeClient::connect().await.unwrap();
    let (object_id, resolved_name) = match resolve_object_arg(&client, &id).await {
        Ok(resolved) => resolved,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };
    match client.get(object_id).await {
        Ok(object) => {
            let data = &object.payload.data;
            let all_text = std::str::from_utf8(data).is_ok();
            let name = object_name(&client, &object.id.0).await.unwrap_or(resolved_name);
            if ids {
                println!("Id: {}", object.id);
            }
            println!("Name: {name}");
            println!("Owner: {}", object.payload.owner);
            println!("Type: {:?}", object.payload.object_type);
            println!("Size: {} bytes", object.payload.metadata.size);
            if let Some(content_type) = &object.payload.metadata.content_type {
                println!("Content type: {content_type}");
            }
            println!("Created: {}", object.payload.metadata.created_at);
            println!();
            if all_text {
                println!("Preview:");
                let text = String::from_utf8_lossy(data);
                let preview: String = text.chars().take(200).collect();
                println!("{preview}");
                if text.chars().count() > 200 {
                    println!("… (truncated)");
                }
            } else {
                println!("Preview: (binary, first bytes: {})", hex_preview(data));
                let printable: Vec<u8> = data
                    .iter()
                    .take(200)
                    .map(|b| if b.is_ascii_graphic() || *b == b' ' { *b } else { b'.' })
                    .collect();
                println!("{}", String::from_utf8_lossy(&printable));
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn list(ids: bool) {
    let client = NodeClient::new().await.unwrap();
    let response = client.request(NodeCommand::List).await.unwrap();

    match response {
        NodeResponse::Objects { objects } => {
            println!("Canopee Objects:\n");
            // User-facing objects are the named ones (`put` / `fetch`
            // name everything stored). Unnamed ones are mostly
            // internal records (home index, profile...) that only
            // clutter a file listing — they surface under `--ids`.
            for object in objects {
                if object.name.is_none() && !ids {
                    continue;
                }
                match &object.name {
                    Some(name) => println!("{name}"),
                    None => println!("(unnamed object)"),
                }
                if ids {
                    println!("Id: {}", object.id);
                }
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

pub(crate) async fn export(id: String) {
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

pub(crate) async fn import(path: String) {
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