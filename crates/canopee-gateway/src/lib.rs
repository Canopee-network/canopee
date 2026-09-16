//! A local bridge that lets a browser tab drive a Canopee node.
//!
//! [`CanopeeClient`] talks to the node over a Unix socket, which a browser
//! sandbox can't reach. This crate puts a small WebSocket server on
//! `127.0.0.1` in front of that socket: browser JavaScript speaks
//! [`GatewayCommand`]/[`GatewayEvent`] JSON over WebSocket, and the gateway
//! translates each command into a [`CanopeeClient`] call against the local
//! node.
//!
//! The gateway is a strictly local trust domain:
//! - it binds loopback only,
//! - it requires a per-process secret [`SessionToken`] in the WebSocket URL,
//! - it rejects handshakes whose `Origin` isn't a loopback origin.
//!
//! It also enforces a *safe command subset*: there is no way to shut the node
//! down, and nothing that exposes the SDK's lower-level administration.
//! Network capabilities stay in the node itself — the gateway is a thin
//! transport, not a new peer.
//!
//! # Examples
//!
//! ```
//! use canopee_gateway::{Gateway, SessionToken};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let token = SessionToken::new();
//!     let gateway = Gateway::start(token.clone()).await?;
//!     println!("{}", gateway.session_url(&token)); // ws://127.0.0.1:<port>/?token=…
//!     gateway.serve().await?;
//!     Ok(())
//! }
//! ```

mod demo;
mod protocol;
mod ws;

pub use protocol::{
    ContactListView, ExportBundleView, GatewayCommand, GatewayEvent, HomeIndexView, ObjectView,
    PointerView, ProfileView,
};
pub use ws::WebSocket;

use anyhow::Context;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use canopee_identity::IdentityId;
use canopee_sdk::CanopeeClient;
use canopee_storage::{AppPointerRecord, ContactList, ExportBundle, HomeIndex, ObjectId, Profile};
use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// A random secret that gates access to a [`Gateway`] instance. Authorized
/// sessions must present it in the WebSocket URL query string.
#[derive(Clone, Debug)]
pub struct SessionToken(pub String);

impl SessionToken {
    /// Generates a fresh 128-bit token.
    pub fn new() -> Self {
        SessionToken(random_hex(16))
    }

    /// The raw hex token, as it appears in the WebSocket URL.
    pub fn token_hex(&self) -> &str {
        &self.0
    }

    /// Constant-time comparison so timing can't leak the token bytes.
    fn matches(&self, candidate: &str) -> bool {
        if self.0.len() != candidate.len() {
            return false;
        }
        let diff = self
            .0
            .as_bytes()
            .iter()
            .zip(candidate.as_bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b));
        diff == 0
    }
}

impl Default for SessionToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads `len` random bytes from the OS CSPRNG and hex-encodes them.
/// Unix sockets are this project's substrate, so `/dev/urandom` is the
/// portable-enough source (with a time-based fallback that should never be
/// hit).
fn random_hex(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    let source = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if source.is_err() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = (nanos >> (i * 8)) as u8 ^ ((i as u128 * 0x9e37_79b9_7f4a_7c15) as u8);
        }
    }
    let mut hex = String::with_capacity(len * 2);
    for byte in bytes {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// The local WebSocket bridge to a Canopee node.
pub struct Gateway {
    addr: SocketAddr,
    token: SessionToken,
}

impl Gateway {
    /// Binds a loopback listener on an ephemeral port. Doesn't start
    /// accepting connections until [`Gateway::serve`] is called.
    pub async fn start(token: SessionToken) -> anyhow::Result<Self> {
        Self::start_on(token, None).await
    }

    /// Like [`Gateway::start`], but binds the given port when one is
    /// requested (falling back to an ephemeral port if it's busy).
    pub async fn start_on(token: SessionToken, port: Option<u16>) -> anyhow::Result<Self> {
        let addr = match port {
            Some(port) => {
                let listener = TcpListener::bind(("127.0.0.1", port))
                    .await
                    .context(format!("failed to bind gateway listener on port {port}"))?;
                let addr = listener.local_addr()?;
                drop(listener);
                addr
            }
            None => {
                let listener = TcpListener::bind(("127.0.0.1", 0))
                    .await
                    .context("failed to bind gateway listener")?;
                let addr = listener.local_addr()?;
                drop(listener);
                addr
            }
        };
        Ok(Gateway { addr, token })
    }

    /// The bound address (always loopback).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The session URL a browser can open with this gateway's token.
    pub fn session_url(&self, token: &SessionToken) -> String {
        format!("ws://{}/?token={}", self.addr, token.0)
    }

    /// The demo page served by the gateway (`GET /`), openable in a browser.
    pub fn page_url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    /// Accepts connections forever, dispatching each to its own task.
    pub async fn serve(self) -> anyhow::Result<()> {
        let listener = TcpListener::bind(self.addr)
            .await
            .context("failed to re-bind gateway listener")?;
        let token = Arc::new(self.token);
        loop {
            let (stream, peer) = listener.accept().await?;
            let token = token.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_client(stream, &token).await {
                    eprintln!("gateway client {peer}: {e}");
                }
            });
        }
    }
}

async fn handle_client(stream: TcpStream, token: &SessionToken) -> anyhow::Result<()> {
    match WebSocket::accept(stream, token).await? {
        ws::Accepted::Served => Ok(()),
        ws::Accepted::Socket(mut ws) => {
            let client = CanopeeClient::connect().await?;
            handle_commands(&mut ws, &client).await
        }
    }
}

enum Incoming {
    Command(anyhow::Result<Option<GatewayCommand>>),
    Subscription(Option<GatewayEvent>),
}

/// Handles one authenticated connection: reads commands, runs them against
/// the node, and streams pub/sub messages while a subscription is active.
/// Multiple sequential `subscribe` commands replace the active subscription;
/// every other command is answered immediately alongside it.
async fn handle_commands(ws: &mut WebSocket, client: &CanopeeClient) -> anyhow::Result<()> {
    let mut sub_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<GatewayEvent>> = None;
    let mut sub_task: Option<JoinHandle<()>> = None;

    loop {
        let incoming = match sub_receiver.as_mut() {
            Some(receiver) => tokio::select! {
                command = ws.read_command() => Incoming::Command(command),
                event = receiver.recv() => Incoming::Subscription(event),
            },
            None => Incoming::Command(ws.read_command().await),
        };

        match incoming {
            Incoming::Command(incoming) => match incoming? {
                Some(GatewayCommand::Subscribe { topic }) => {
                    if let Some(task) = sub_task.take() {
                        task.abort();
                    }
                    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
                    sub_receiver = Some(receiver);
                    sub_task = Some(tokio::spawn(async move {
                        pump_subscription(topic, sender).await;
                    }));
                }
                Some(command) => {
                    let event = dispatch(client, command).await;
                    ws.reply(&event).await?;
                }
                None => return Ok(()),
            },
            Incoming::Subscription(event) => match event {
                Some(event) => ws.reply(&event).await?,
                None => return Ok(()),
            },
        }
    }
}

/// Streams a live subscription's messages into `sender`, ending when the
/// node closes the subscription (or the browser's socket is gone).
async fn pump_subscription(
    topic: String,
    sender: tokio::sync::mpsc::UnboundedSender<GatewayEvent>,
) {
    // Each pump opens its own client; like the node, every subscription is
    // its own connection to the Unix socket.
    let Ok(client) = CanopeeClient::connect().await else {
        let _ = sender.send(GatewayEvent::error(
            "failed to connect to the node for subscription".to_string(),
        ));
        return;
    };
    let mut subscription = match client.subscribe(&topic).await {
        Ok(subscription) => subscription,
        Err(e) => {
            let _ = sender.send(GatewayEvent::error(format!(
                "failed to subscribe to \"{topic}\": {e}"
            )));
            return;
        }
    };
    while let Ok(Some(message)) = subscription.next().await {
        if sender
            .send(GatewayEvent::PubSub {
                topic: message.topic,
                source: message.source,
                text: String::from_utf8_lossy(&message.data).to_string(),
                data_b64: base64_encode(&message.data),
            })
            .is_err()
        {
            return;
        }
    }
}

/// Routes a single command to the corresponding [`CanopeeClient`] method and
/// shapes the result into a JSON-friendly event.
async fn dispatch(client: &CanopeeClient, command: GatewayCommand) -> GatewayEvent {
    match handle(client, command).await {
        Ok(result) => GatewayEvent::ok(result),
        Err(e) => GatewayEvent::error(e.to_string()),
    }
}

/// Runs one command against the node and returns its JSON result.
async fn handle(
    client: &CanopeeClient,
    command: GatewayCommand,
) -> anyhow::Result<serde_json::Value> {
    use GatewayCommand::*;
    match command {
        Identity => {
            let identity = client.identity().await?;
            Ok(serde_json::json!({ "identity": identity.to_string() }))
        }

        Put { text } => {
            let id = client.put(text.into_bytes()).await?;
            Ok(serde_json::json!({ "id": id.0 }))
        }

        Get { id } => {
            let object = client.get(ObjectId::new(&id)).await?;
            Ok(serde_json::json!({ "object": ObjectView::from(&object) }))
        }

        List => {
            let objects = client.list().await?;
            let views = objects
                .into_iter()
                .map(|info| {
                    serde_json::json!({
                        "id": info.id.0,
                        "owner": info.owner.to_string(),
                        "size": info.size,
                        "verified": info.verified,
                    })
                })
                .collect::<Vec<_>>();
            Ok(serde_json::json!({ "objects": views }))
        }

        Export { id } => {
            let bundle = client.export(ObjectId::new(&id)).await?;
            Ok(serde_json::json!({ "object": ObjectView::from(&bundle.object) }))
        }

        Import { bundle } => {
            let object = bundle.reconstruct()?;
            let bundle = ExportBundle {
                version: bundle.version,
                object,
            };
            client.import(bundle).await?;
            Ok(serde_json::json!({}))
        }

        Peers => {
            let peers = client.peers().await?;
            let views = peers
                .into_iter()
                .map(|peer| {
                    serde_json::json!({
                        "peerId": peer.peer_id,
                        "identity": peer.identity.map(|i| i.to_string()),
                        "username": peer.username,
                        "displayName": peer.display_name,
                        "addresses": peer.addresses,
                    })
                })
                .collect::<Vec<_>>();
            Ok(serde_json::json!({ "peers": views }))
        }

        Dial { addr } => {
            client.dial(addr).await?;
            Ok(serde_json::json!({}))
        }

        Announce { id } => {
            client.announce(ObjectId::new(&id)).await?;
            Ok(serde_json::json!({}))
        }

        FindProviders { id } => {
            let providers = client.find_providers(ObjectId::new(&id)).await?;
            Ok(serde_json::json!({ "peerIds": providers }))
        }

        FetchObject { peer_id, id } => {
            let bundle = client.fetch_object(peer_id, ObjectId::new(&id)).await?;
            Ok(serde_json::json!({ "object": ObjectView::from(&bundle.object) }))
        }

        Publish { topic, text } => {
            client.publish(topic, text.into_bytes()).await?;
            Ok(serde_json::json!({}))
        }

        PublishAppPointer { name, manifest } => {
            client
                .publish_app_pointer(name, ObjectId::new(&manifest))
                .await?;
            Ok(serde_json::json!({}))
        }

        ResolveAppPointer { owner, name } => {
            let manifest = client
                .resolve_app_pointer(IdentityId::new(owner), name)
                .await?;
            Ok(serde_json::json!({ "manifest": manifest.map(|m| m.0) }))
        }

        PublishPointer { name, target } => {
            client.publish_pointer(name, ObjectId::new(&target)).await?;
            Ok(serde_json::json!({}))
        }

        ResolvePointer { owner, name } => {
            let pointer = client.resolve_pointer(IdentityId::new(owner), name).await?;
            Ok(serde_json::json!({ "pointer": pointer.as_ref().map(pointer_view) }))
        }

        LoadProfile => {
            let profile = client.load_profile().await?;
            Ok(serde_json::json!({ "profile": profile.map(|p| ProfileView::from(&p)) }))
        }

        SaveProfile { profile } => {
            let id = client.save_profile(&Profile::try_from(profile)?).await?;
            Ok(serde_json::json!({ "id": id.0 }))
        }

        LoadContactList => {
            let list = client.load_contact_list().await?;
            Ok(serde_json::json!({ "list": list.map(|l| ContactListView::from(&l)) }))
        }

        SaveContactList { list } => {
            let id = client
                .save_contact_list(&ContactList::try_from(list)?)
                .await?;
            Ok(serde_json::json!({ "id": id.0 }))
        }

        LoadHomeIndex => {
            let index = client.load_home_index().await?;
            Ok(serde_json::json!({ "index": index.map(|i| HomeIndexView::from(&i)) }))
        }

        SaveHomeIndex { index } => {
            let id = client.save_home_index(&HomeIndex::try_from(index)?).await?;
            Ok(serde_json::json!({ "id": id.0 }))
        }

        SetHomeEntryShared { name, shared } => {
            let id = client.set_home_entry_shared(name, shared).await?;
            Ok(serde_json::json!({ "id": id.0 }))
        }

        ShareObject { name, object, app } => {
            let id = client
                .share_object(name, ObjectId::new(&object), app)
                .await?;
            Ok(serde_json::json!({ "id": id.0 }))
        }

        ClaimUsername { username } => {
            client.claim_username(username).await?;
            Ok(serde_json::json!({}))
        }

        ShowUsername => {
            let username = client.show_username().await?;
            Ok(serde_json::json!({ "username": username }))
        }

        ResolveUsername { username } => {
            let owner = client.resolve_username(username).await?;
            Ok(serde_json::json!({ "owner": owner.map(|o| o.to_string()) }))
        }

        ExportIdentity { passphrase } => {
            let bytes = client.export_identity(passphrase).await?;
            Ok(serde_json::json!({ "dataB64": base64_encode(&bytes) }))
        }

        ImportIdentity {
            data_b64,
            passphrase,
            overwrite,
        } => {
            let bytes = B64
                .decode(data_b64)
                .map_err(|e| anyhow::anyhow!("invalid base64 dataB64: {e}"))?;
            let identity_id = client.import_identity(bytes, passphrase, overwrite).await?;
            Ok(serde_json::json!({
                "identityId": identity_id.to_string(),
                "restartRequired": true,
            }))
        }

        // Intercepted in `handle_commands` before reaching here, but the
        // match must be exhaustive.
        Subscribe { .. } => anyhow::bail!("subscribe must be handled as a streaming connection"),
    }
}

fn pointer_view(record: &AppPointerRecord) -> PointerView {
    protocol::pointer_to_view(
        record.owner.to_string(),
        record.name.clone(),
        &record.manifest,
        record.published_at,
    )
}

fn base64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
