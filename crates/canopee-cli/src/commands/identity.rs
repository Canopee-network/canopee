use canopee_protocol::{NodeCommand, NodeResponse};

pub(crate) async fn identity() {
    let client = super::common::node_or_exit().await;
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

pub(crate) async fn dh_key() {
    use base64::Engine as _;
    let client = super::common::client_or_exit().await;
    match client.dh_public_key().await {
        Ok(key) => println!("{}", base64::engine::general_purpose::STANDARD.encode(key)),
        Err(e) => eprintln!("Error: {e}"),
    }
}

pub(crate) async fn device() {
    let client = super::common::client_or_exit().await;
    match client.device().await {
        Ok((peer_id, device_name)) => {
            println!("{peer_id}");
            println!("{device_name}");
        }
        Err(e) => eprintln!("Error: {e}"),
    }
}

pub(crate) async fn devices() {
    let client = super::common::client_or_exit().await;
    match client.device_list().await {
        Ok(devices) => {
            if devices.is_empty() {
                println!("No devices registered for this identity yet");
            }
            for device in devices {
                println!("{} ({})", device.device_id, device.device_name);
            }
        }
        Err(e) => eprintln!("Error: {e}"),
    }
}

pub(crate) async fn profile(name: Option<String>) {
    let client = super::common::client_or_exit().await;
    match name {
        None => match client.load_profile().await {
            Ok(Some(profile)) => {
                use base64::Engine as _;
                println!("Display name: {}", profile.display_name);
                println!("Version: {}", profile.version);
                // Public, and needed to exchange contacts: the other side
                // feeds this to `canopee contact add` so you can share
                // objects they can decrypt.
                println!(
                    "DH key: {}",
                    base64::engine::general_purpose::STANDARD.encode(profile.dh_public_key)
                );
            }
            Ok(None) => println!("No profile set yet"),
            Err(e) => eprintln!("Error: {e}"),
        },
        Some(name) => {
            let current = client.load_profile().await.unwrap_or(None);
            let profile = canopee_storage::Profile {
                display_name: name,
                // The DH key belongs to the identity, not to the profile, so it
                // must always be the live one. Publishing an all-zero key here
                // would make us unshareable: peers would wrap the content key
                // for a key nobody holds, and decryption would silently fail.
                dh_public_key: client.dh_public_key().await.unwrap_or_else(|e| {
                    eprintln!("Error: could not read this node's DH key: {e}");
                    std::process::exit(1);
                }),
                avatar: current.and_then(|p| p.avatar),
                version: 0, // overwritten by the runtime
            };
            match client.save_profile(&profile).await {
                Ok(id) => println!("Profile saved: {id}"),
                Err(e) => eprintln!("Error: {e}"),
            }
        }
    }
}

pub(crate) async fn username(command: crate::UsernameCommand) {
    let client = super::common::client_or_exit().await;
    match command {
        crate::UsernameCommand::Claim { username } => {
            match client.claim_username(&username).await {
                Ok(()) => println!("Claimed username \"{username}\""),
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
        crate::UsernameCommand::Show => match client.show_username().await {
            Ok(Some(username)) => println!("{username}"),
            Ok(None) => {
                println!("No username claimed yet (claim one with `canopee username claim <name>`)")
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        },
        crate::UsernameCommand::Lookup { username } => {
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

pub(crate) async fn export_identity(passphrase: String, output: String) {
    let client = super::common::client_or_exit().await;
    match client.export_identity(passphrase).await {
        Ok(bytes) => {
            tokio::fs::write(&output, &bytes).await.unwrap();
            println!(
                "Wrote {} encrypted identity bytes to {}",
                bytes.len(),
                output
            );
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn import_identity(path: String, passphrase: String, overwrite: bool) {
    let bytes = tokio::fs::read(&path).await.unwrap();
    let client = super::common::client_or_exit().await;
    match client.import_identity(bytes, passphrase, overwrite).await {
        Ok(identity_id) => {
            println!("Imported identity {identity_id}");
            println!("Restart the node to adopt it.");
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}
