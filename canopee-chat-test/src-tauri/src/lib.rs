//! Tauri backend: embeds a `canopee_runtime::Runtime` in-process (tutorial
//! steps 2–4, 6) and bridges it to a web UI via `#[tauri::command]`s and
//! `app.emit` events. No separate `canopee-node` process, no Unix socket.

pub mod chat;

use chat::{format_contact, parse_contact, Contact, Conversation};
use canopee_storage::{ContactList, HomeEntry, HomeIndex, ObjectType};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::RwLock;

use canopee_runtime::Runtime;

/// Own conversation data (deterministic; see `Conversation`) plus the live
/// node. Shared across all command invocations via Tauri managed state.
struct AppState {
    runtime: Arc<Runtime>,
    conversations: RwLock<HashMap<String, Conversation>>,
}

#[derive(serde::Serialize)]
struct SelfInfo {
    /// Full shareable contact string, e.g.
    /// `canopee://identity/<peer-id>#dh=<hex>`. Copy this and send it to a
    /// contact out of band.
    contact: String,
    peer_id: String,
    dh_public_key: String,
    listen_addrs: Vec<String>,
    peers_connected: usize,
}

#[derive(serde::Serialize, Clone)]
struct ConversationInfo {
    name: String,
    peer_id: String,
    topic: String,
}

#[derive(serde::Serialize, Clone)]
struct IncomingMessage {
    from: String,
    text: String,
}

/// One row of the user's shared "my data" view: a file/picture they put in
/// their store, read from the signed `HomeIndex` object the same way any of
/// their other apps would read it.
#[derive(serde::Serialize, Clone)]
struct MyDataInfo {
    name: String,
    object_id: String,
    size: u64,
    object_type: String,
    shared: bool,
    app: Option<String>,
}

#[derive(serde::Serialize)]
struct NetworkPeer {
    peer_id: String,
    identity: Option<String>,
    addresses: Vec<String>,
}

#[derive(serde::Deserialize)]
struct AddPeerArgs {
    name: String,
    contact: String,
}

#[derive(serde::Deserialize)]
struct SendArgs {
    name: String,
    text: String,
}

impl AppState {
    fn peer_id(&self) -> String {
        self.runtime
            .identity
            .keypair()
            .public()
            .to_peer_id()
            .to_base58()
    }
}

// ---- commands ----

/// Tutorial step 2's verification command: the app's own identity, as a
/// shareable string, with no separate node process anywhere.
#[tauri::command]
async fn get_my_identity(state: State<'_, AppState>) -> Result<String, String> {
    let dh = state.runtime.identity.dh_public_key();
    Ok(format_contact(&state.peer_id(), &dh))
}

#[tauri::command]
async fn get_self(state: State<'_, AppState>) -> Result<SelfInfo, String> {
    let peer_id = state.peer_id();
    let dh = state.runtime.identity.dh_public_key();
    let listen_addrs = state
        .runtime
        .network
        .listen_addresses()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|a| a.to_string())
        .collect();
    let peers_connected = state.runtime.network.peers().await.map_err(|e| e.to_string())?.len();
    Ok(SelfInfo {
        contact: format_contact(&peer_id, &dh),
        peer_id,
        dh_public_key: hex::encode(dh),
        listen_addrs,
        peers_connected,
    })
}

/// Step 3: registers a contact (their shareable string) and opens the
/// per-conversation gossipsub topic for live messaging (steps 4 + 6c).
#[tauri::command]
async fn add_peer(
    app: AppHandle,
    state: State<'_, AppState>,
    args: AddPeerArgs,
) -> Result<ConversationInfo, String> {
    if args.name.trim().is_empty() {
        return Err("name must not be empty".to_string());
    }
    if state.conversations.read().await.contains_key(&args.name) {
        return Err(format!("a contact named '{}' already exists", args.name));
    }

    let dh_public_key = parse_contact(&args.contact)?;
    // Peer id is the `canopee://identity/...` segment for display, and is
    // what a dial target multiaddr would carry.
    let peer_id = args
        .contact
        .strip_prefix("canopee://identity/")
        .and_then(|rest| rest.split('#').next())
        .unwrap_or_default()
        .to_string();

    let conversation = Conversation::new(
        &state.runtime.identity,
        Contact {
            name: args.name.clone(),
            peer_id,
            dh_public_key,
        },
    );

    // Persist into the user's shared contact list *first* (merge: upsert by
    // name, keep every other contact). This object lives at ~/.canopee, so
    // the user's other apps — and a new device after key import — see it too.
    let mut list = state
        .runtime
        .load_contact_list()
        .await
        .ok()
        .flatten()
        .unwrap_or(ContactList {
            contacts: Vec::new(),
            version: 0,
        });
    let user_contact = conversation.contact.to_user_contact();
    if let Some(existing) = list.contacts.iter_mut().find(|c| c.name == user_contact.name) {
        *existing = user_contact;
    } else {
        list.contacts.push(user_contact);
    }
    state
        .runtime
        .save_contact_list(&list)
        .await
        .map_err(|e| format!("failed to persist contacts: {e}"))?;

    state
        .conversations
        .write()
        .await
        .insert(args.name.clone(), conversation.clone());

    spawn_receiver(
        app,
        state.runtime.clone(),
        state.runtime.identity.keypair().public().to_peer_id(),
        conversation.clone(),
    );

    Ok(ConversationInfo {
        name: conversation.contact.name.clone(),
        peer_id: conversation.contact.peer_id.clone(),
        topic: conversation.topic().to_string(),
    })
}

/// Step 4 / 6c: encrypts and publishes a message to the recipient.
#[tauri::command]
async fn send_message(state: State<'_, AppState>, args: SendArgs) -> Result<(), String> {
    if args.text.trim().is_empty() {
        return Err("message must not be empty".to_string());
    }
    let conversation = state
        .conversations
        .read()
        .await
        .get(&args.name)
        .cloned()
        .ok_or_else(|| format!("no contact named '{}'", args.name))?;

    let ciphertext = conversation.encrypt(args.text.as_bytes()).map_err(|e| e.to_string())?;
    state
        .runtime
        .network
        .publish(conversation.topic(), ciphertext)
        .await
        .map_err(|e| e.to_string())
}

/// Manual peek at connected libp2p peers (e.g. to confirm mDNS discovery).
#[tauri::command]
async fn get_peers(state: State<'_, AppState>) -> Result<Vec<NetworkPeer>, String> {
    let peers = state.runtime.network.peers().await.map_err(|e| e.to_string())?;
    Ok(peers
        .into_iter()
        .map(|p| NetworkPeer {
            peer_id: p.peer_id.to_string(),
            identity: p.identity.map(|i| i.to_string()),
            addresses: p.addresses.into_iter().map(|a| a.to_string()).collect(),
        })
        .collect())
}

/// Directly dials a peer (same-LAN manual fallback; mDNS usually handles it).
#[tauri::command]
async fn dial(state: State<'_, AppState>, addr: String) -> Result<(), String> {
    let addr = addr
        .parse()
        .map_err(|e| format!("invalid multiaddr {addr:?}: {e}"))?;
    state
        .runtime
        .network
        .dial(addr)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_conversations(state: State<'_, AppState>) -> Result<Vec<ConversationInfo>, String> {
    Ok(state
        .conversations
        .read()
        .await
        .values()
        .map(|c| ConversationInfo {
            name: c.contact.name.clone(),
            peer_id: c.contact.peer_id.clone(),
            topic: c.topic().to_string(),
        })
        .collect())
}

/// Stores a picture/file the user has in their store: the bytes go into a
/// signed, content-addressed `Blob` object and are listed in the shared
/// `HomeIndex` (the user's "table of contents" across all their apps).
#[tauri::command]
async fn attach_picture(state: State<'_, AppState>, path: String) -> Result<MyDataInfo, String> {
    if path.trim().is_empty() {
        return Err("path must not be empty".to_string());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("can't read {path}: {e}"))?;
    let name = Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    let object = state
        .runtime
        .put_object(bytes.clone(), ObjectType::Blob)
        .await
        .map_err(|e| e.to_string())?;
    let mut index = state
        .runtime
        .load_home_index()
        .await
        .map_err(|e| e.to_string())?
        .unwrap_or(HomeIndex {
            profile: None,
            contacts: None,
            entries: Vec::new(),
            version: 0,
        });
    index.entries.push(HomeEntry {
        name: name.clone(),
        object: object.id.clone(),
        object_type: ObjectType::Blob,
        shared: false,
        app: Some("chat".to_string()),
    });
    state
        .runtime
        .save_home_index(&index)
        .await
        .map_err(|e| format!("failed to persist home index: {e}"))?;
    Ok(MyDataInfo {
        name,
        object_id: object.id.0.clone(),
        size: bytes.len() as u64,
        object_type: "blob".to_string(),
        shared: false,
        app: Some("chat".to_string()),
    })
}

/// The user's shared "my data" view: everything their apps have put in the
/// store, as listed in the signed `HomeIndex` object.
#[tauri::command]
async fn my_data(state: State<'_, AppState>) -> Result<Vec<MyDataInfo>, String> {
    let Some(index) = state
        .runtime
        .load_home_index()
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(vec![]);
    };
    let mut out = Vec::new();
    for entry in index.entries {
        let size = match state.runtime.get(&entry.object).await {
            Ok(o) => o.payload.metadata.size,
            Err(_) => 0,
        };
        out.push(MyDataInfo {
            name: entry.name,
            object_id: entry.object.0.clone(),
            size,
            object_type: match entry.object_type {
                ObjectType::Blob => "blob".to_string(),
                _ => "unknown".to_string(),
            },
            shared: entry.shared,
            app: entry.app,
        });
    }
    Ok(out)
}

// ---- background receiver loop ----

/// Subscribes to a conversation's topic and forwards decrypted messages to
/// the frontend as `chat-message` events (tutorial step 4 + 6c).
fn spawn_receiver(
    app: AppHandle,
    runtime: Arc<Runtime>,
    own_peer_id: canopee_network::PeerId,
    conversation: Conversation,
) {
    let topic = conversation.topic().to_string();
    let name = conversation.contact.name.clone();
    tauri::async_runtime::spawn(async move {
        let mut receiver = match runtime.network.subscribe(&topic).await {
            Ok(rx) => rx,
            Err(e) => {
                eprintln!("[chat] failed to subscribe to {topic}: {e}");
                return;
            }
        };
        loop {
            match receiver.recv().await {
                Ok(msg) => {
                    if msg.topic != topic {
                        continue;
                    }
                    // Defensive self-filter. libp2p 0.54 gossipsub does not redeliver
                    // locally published messages, and the frontend already
                    // renders our own sends, so a reflected echo would only
                    // duplicate them.
                    if msg.source.as_ref() == Some(&own_peer_id) {
                        continue;
                    }
                    let Some(plaintext) = conversation.decrypt(&msg.data) else {
                        continue;
                    };
                    let text = String::from_utf8_lossy(&plaintext).to_string();
                    let _ = app.emit(
                        "chat-message",
                        IncomingMessage {
                            from: name.clone(),
                            text,
                        },
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break, // sender dropped
            }
        }
    });
}

// ---- entry point ----

/// Opens the embedded runtime rooted at the shared user store.
///
/// Identity + user data live in the user root (default `~/.canopee`), so any
/// of the user's apps — and any new device after key import — sees the same
/// identity and the same contacts/files. The app's own state/cache still
/// lives under the per-app data dir.
///
/// `CANOPEE_USER_ROOT` overrides the user root (testing two identities on one
/// machine, or simulating a fresh device with a new store).
async fn open_runtime(data_dir: PathBuf) -> anyhow::Result<Runtime> {
    match std::env::var("CANOPEE_USER_ROOT") {
        Ok(user_root) => {
            let config = canopee_config::Config::new()
                .with_app_root(data_dir)
                .with_user_root(PathBuf::from(user_root))
                .with_mdns(false);
            Runtime::open_with_config(config).await
        }
        Err(_) => Runtime::open_with_user_root(data_dir).await,
    }
}

/// Restores the in-memory address book from the user's shared `ContactList`
/// object, so restarting the app (or opening any of the user's other apps)
/// finds their contacts again. Each contact gets its gossipsub receiver
/// re-subscribed.
fn seed_conversations(app: &tauri::App, runtime: &Arc<Runtime>) {
    let list = match tauri::async_runtime::block_on(runtime.load_contact_list()) {
        Ok(Some(l)) => l,
        _ => return,
    };
    let own_peer_id = runtime.identity.keypair().public().to_peer_id();
    let state = app.state::<AppState>();
    let mut conversations = tauri::async_runtime::block_on(state.conversations.write());
    for user_contact in list.contacts {
        let conversation = Conversation::new(&runtime.identity, Contact::from(user_contact));
        conversations.insert(conversation.contact.name.clone(), conversation.clone());
        spawn_receiver(
            app.handle().clone(),
            runtime.clone(),
            own_peer_id,
            conversation,
        );
    }
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let runtime = tauri::async_runtime::block_on(open_runtime(data_dir))?;
            let state = AppState {
                runtime: Arc::new(runtime),
                conversations: RwLock::new(HashMap::new()),
            };
            app.manage(state);
            seed_conversations(app, &app.state::<AppState>().runtime);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_my_identity,
            get_self,
            add_peer,
            send_message,
            get_peers,
            dial,
            list_conversations,
            attach_picture,
            my_data,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}