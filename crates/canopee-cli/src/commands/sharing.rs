use crate::commands::common::resolve_object_arg;
use canopee_sdk::CanopeeClient;

pub(crate) async fn share(name: String, id: Option<String>, to: Vec<String>, ids: bool) {
    let client = super::common::client_or_exit().await;
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

    let recipients = match recipient_keys(&client, &to).await {
        Ok(keys) => keys,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    if ids {
        client
            .share_object_for(
                name.clone(),
                object_id.clone(),
                Some("cli".into()),
                &recipients,
            )
            .await
            .unwrap();
        println!("Shared \"{name}\"");
        println!("Id: {object_id}");
        if !recipients.is_empty() {
            // The published copy is re-encrypted per recipient, so it has its
            // own id; say so rather than printing the local one as if it were
            // what got published.
            println!(
                "Shared with {} contact(s) as a separate encrypted copy; \
                 the id above is your local original.",
                recipients.len()
            );
        }
    } else {
        client
            .share_object_for(
                name.clone(),
                object_id.clone(),
                Some("cli".into()),
                &recipients,
            )
            .await
            .unwrap();
        println!("Shared \"{name}\"");
    }
}

/// Resolves each `--to` argument to a contact's X25519 DH public key.
///
/// Accepts either the contact's name or its peer id, because a user knows
/// people by name but the network knows them by peer id.
async fn recipient_keys(
    client: &CanopeeClient,
    wanted: &[String],
) -> anyhow::Result<Vec<[u8; 32]>> {
    if wanted.is_empty() {
        return Ok(vec![]);
    }
    let contacts = client.load_contact_list().await?.ok_or_else(|| {
        anyhow::anyhow!(
            "--to needs contacts, but you have no contact list yet. \
             Add the person with `canopee contact add <name> <peer-id> <dh-key>` first."
        )
    })?;

    let mut keys = Vec::with_capacity(wanted.len());
    for arg in wanted {
        let contact = contacts
            .contacts
            .iter()
            .find(|c| c.name == *arg)
            .or_else(|| contacts.contacts.iter().find(|c| c.peer_id == *arg))
            .ok_or_else(|| {
                let known: Vec<&str> = contacts.contacts.iter().map(|c| c.name.as_str()).collect();
                anyhow::anyhow!(
                    "no contact matches {arg:?}. Known contacts: {}",
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                )
            })?;
        keys.push(contact.dh_public_key);
    }
    Ok(keys)
}

pub(crate) async fn unshare(name: String) {
    let client = super::common::client_or_exit().await;
    match client.unshare(name.clone()).await {
        Ok(_) => println!("Unshared \"{}\"", name),
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

pub(crate) async fn home(ids: bool) {
    let client = super::common::client_or_exit().await;
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
    let client = super::common::client_or_exit().await;
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

/// Records a contact so `canopee share --to <name>` can find their DH key.
pub(crate) async fn contact_add(name: String, peer_id: String, dh_key_b64: String) {
    use base64::Engine as _;
    use canopee_storage::{Contact, ContactList};

    let client = super::common::client_or_exit().await;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(dh_key_b64.trim()) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("Error: the DH key must be base64: {e}");
            std::process::exit(1);
        }
    };
    let dh_public_key: [u8; 32] = match bytes.try_into() {
        Ok(key) => key,
        Err(_) => {
            eprintln!("Error: the DH key must decode to exactly 32 bytes");
            std::process::exit(1);
        }
    };

    let mut list = client
        .load_contact_list()
        .await
        .unwrap_or(None)
        .unwrap_or(ContactList {
            contacts: vec![],
            version: 0,
        });
    match list.contacts.iter_mut().find(|c| c.name == name) {
        Some(existing) => {
            existing.peer_id = peer_id;
            existing.dh_public_key = dh_public_key;
        }
        None => list.contacts.push(Contact {
            name: name.clone(),
            peer_id,
            dh_public_key,
            note: None,
        }),
    }
    list.version += 1;

    if let Err(e) = client.save_contact_list(&list).await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
    println!("Contact \"{name}\" saved");
}
