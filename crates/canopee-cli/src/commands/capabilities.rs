use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use canopee_sdk::CanopeeClient;
use canopee_storage::{Permission, Resource};

pub(crate) async fn capabilities(command: crate::CapCommand) {
    let client = CanopeeClient::connect().await.unwrap();
    match command {
        crate::CapCommand::Grant {
            subject,
            resource,
            read,
            write,
            publish,
            expires,
        } => {
            let mut permissions = Vec::new();
            if read {
                permissions.push(Permission::Read);
            }
            if write {
                permissions.push(Permission::Write);
            }
            if publish {
                permissions.push(Permission::Publish);
            }
            if permissions.is_empty() {
                eprintln!("Error: grant at least one of --read, --write, --publish");
                std::process::exit(1);
            }
            match client
                .grant_capability(
                    canopee_sdk::IdentityId::new(subject),
                    Resource::parse(&resource).unwrap_or_else(|e| {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }),
                    permissions,
                    expires,
                )
                .await
            {
                Ok(cap) => {
                    println!(
                        "Granted {} on {} to {}",
                        cap.id,
                        cap.resource.canonical(),
                        cap.subject
                    );
                    if let Some(exp) = cap.expires_at {
                        println!("Expires: {exp}");
                    }
                    // Print the transferable bundle so it can be handed
                    // to the subject over any channel.
                    let encoded =
                        BASE64.encode(bincode::serialize(&cap.export().unwrap()).unwrap());
                    println!("Capability bundle (base64, for the subject):");
                    println!("{encoded}");
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
        crate::CapCommand::List => match client.list_capabilities().await {
            Ok(Some(index)) => {
                if index.entries.is_empty() {
                    println!("No capabilities issued yet");
                }
                for entry in index.entries {
                    let cap = &entry.capability;
                    println!(
                        "{} (revoked: {})",
                        cap.id,
                        if entry.revoked { "yes" } else { "no" }
                    );
                    println!(
                        "  {} -> {} on {}",
                        cap.issuer,
                        cap.subject,
                        cap.resource.canonical()
                    );
                    let mut perms = cap
                        .permissions
                        .iter()
                        .map(|p| p.as_str().to_string())
                        .collect::<Vec<_>>();
                    perms.sort();
                    println!("  permissions: {}", perms.join(", "));
                    println!(
                        "  expires: {}",
                        cap.expires_at
                            .map(|t| t.to_string())
                            .unwrap_or_else(|| "never".into())
                    );
                }
            }
            Ok(None) => println!("No capabilities issued yet"),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        },
        crate::CapCommand::Revoke { id } => {
            match client
                .revoke_capability(&canopee_sdk::CapabilityId(id.clone()))
                .await
            {
                Ok(()) => println!("Revoked capability {id}"),
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
        crate::CapCommand::Check {
            bundle,
            subject,
            permission,
            resource,
        } => {
            if let Some(bundle_b64) = bundle {
                // End-to-end verification of a presented bundle.
                let bytes = match BASE64.decode(bundle_b64.trim()) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        eprintln!("Error: invalid base64 bundle: {e}");
                        std::process::exit(1);
                    }
                };
                let exported: canopee_storage::ExportCapability = match bincode::deserialize(&bytes)
                {
                    Ok(exported) => exported,
                    Err(e) => {
                        eprintln!("Error: cannot decode capability bundle: {e}");
                        std::process::exit(1);
                    }
                };
                match client.check_capability(&exported.capability).await {
                    Ok((valid, reason)) => {
                        println!("{}: {reason}", if valid { "VALID" } else { "INVALID" });
                        if !valid {
                            std::process::exit(1);
                        }
                    }
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                // Issuer-side authorization check.
                let subject = subject.unwrap_or_else(|| {
                    eprintln!(
                        "Error: pass a bundle, or --subject with --permission and --resource"
                    );
                    std::process::exit(1);
                });
                let permission = permission.unwrap_or_else(|| {
                    eprintln!("Error: --permission required (read|write|publish)");
                    std::process::exit(1);
                });
                let resource = resource.unwrap_or_else(|| {
                    eprintln!("Error: --resource required (object:<id>|channel:<topic>|shared:<owner>:<name>)");
                    std::process::exit(1);
                });
                let permission = Permission::parse(&permission).unwrap_or_else(|| {
                    eprintln!("Error: unknown permission \"{permission}\"");
                    std::process::exit(1);
                });
                match client
                    .check_access(
                        &canopee_sdk::IdentityId::new(subject),
                        permission,
                        &Resource::parse(&resource).unwrap_or_else(|e| {
                            eprintln!("Error: {e}");
                            std::process::exit(1);
                        }),
                    )
                    .await
                {
                    Ok(true) => println!("ACCESS GRANTED"),
                    Ok(false) => {
                        println!("ACCESS DENIED");
                        std::process::exit(1);
                    }
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            }
        }
    }
}
