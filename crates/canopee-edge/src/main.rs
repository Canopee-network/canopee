//! A Canopee edge: the public relay that turns a publisher's local node into a
//! reachable website.
//!
//! Registering publisher nodes verify that the signed `username` claim belongs
//! to the identity listed in the DHT registry (`username:<name>` → owner), so
//! an edge only pins `username.<domain>` traffic to the connection of the
//! username's genuine owner. The edge then proxies plain HTTP request/response
//! packets between browsers and publishers over the existing libp2p
//! connections (via `/canopee/serve/1.0.0`).
//!
//! # Startup
//!
//! Prints parseable lines so an operator or test can discover the edge:
//!
//! ```text
//! Edge peer id: 12D3KooW...
//! Edge listens on: /ip4/0.0.0.0/tcp/4002/p2p/12D3KooW...
//! Edge serving https on 443
//! ```
//!
//! # Environment
//!
//! * `CANOPEE_EDGE_LISTEN_PORT` — libp2p listen port (default `4002`)
//! * `CANOPEE_EDGE_HTTP_PORT` — public HTTP port (default `8080`)
//! * `CANOPEE_EDGE_TLS_CERT` / `CANOPEE_EDGE_TLS_KEY` — PEM cert+key; when both
//!   are set the edge terminates TLS on `CANOPEE_EDGE_HTTP_PORT`
//! * `CANOPEE_EDGE_ROOT` — directory for the edge's persistent key
//!   (default `~/.canopee-edge`); the keypair is kept so the peer id is stable
//! * `CANOPEE_BOOTSTRAP_ADDRS` — passed through to the network layer

use canopee_edge::httpd;
use canopee_network::{NetworkManager, ObjectProvider, PeerId};
use canopee_storage::{ExportBundle, ObjectId};
use std::sync::Arc;

/// The edge never hosts content of its own; requests are always forwarded to a
/// registered publisher. An empty object provider keeps the object-exchange
/// protocol working (publishers dial our identity) while serving nothing.
#[derive(Clone)]
struct EmptyProvider;

#[async_trait::async_trait]
impl ObjectProvider for EmptyProvider {
    async fn get_object(&self, _id: &ObjectId) -> Option<ExportBundle> {
        None
    }
}

fn env_port(name: &str, default: u16) -> u16 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn edge_root() -> std::path::PathBuf {
    std::env::var("CANOPEE_EDGE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_else(|_| ".".to_string());
            std::path::Path::new(&home).join(".canopee-edge")
        })
}

/// Loads the edge's persistent keypair (so its peer id is stable across
/// restarts), generating and persisting a fresh one on first run.
fn edge_key() -> anyhow::Result<libp2p::identity::Keypair> {
    let root = edge_root();
    let key_path = root.join("edge-key.pem");
    if key_path.exists() {
        let bytes = std::fs::read(&key_path)?;
        match libp2p::identity::Keypair::from_protobuf_encoding(&bytes) {
            Ok(keypair) => return Ok(keypair),
            Err(e) => tracing::warn!(
                "failed to load edge key at {} ({e}); generating a fresh one",
                key_path.display()
            ),
        }
    }
    let keypair = libp2p::identity::Keypair::generate_ed25519();
    std::fs::create_dir_all(&root)?;
    std::fs::write(&key_path, keypair.to_protobuf_encoding()?)?;
    Ok(keypair)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var_os("RUST_LOG").is_some() {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .init();
    }

    let keypair = edge_key()?;
    let peer_id = PeerId::from(keypair.public());

    let listen_port = env_port("CANOPEE_EDGE_LISTEN_PORT", 4002);
    let listen_addr: canopee_network::Multiaddr =
        format!("/ip4/0.0.0.0/tcp/{listen_port}").parse()?;

    let network = NetworkManager::new(keypair, listen_addr, Arc::new(EmptyProvider), false)?;

    let http_port = env_port("CANOPEE_EDGE_HTTP_PORT", 8080);
    let tls_cert = std::env::var("CANOPEE_EDGE_TLS_CERT").ok();
    let tls_key = std::env::var("CANOPEE_EDGE_TLS_KEY").ok();
    let tls = match (tls_cert, tls_key) {
        (Some(cert_path), Some(key_path)) => Some(httpd::tls_server_config(&cert_path, &key_path)?),
        _ => None,
    };

    // Parseable startup lines (what the e2e test greps for).
    println!("Edge peer id: {peer_id}");
    for addr in network.listen_addresses().await? {
        println!("Edge listens on: {addr}/p2p/{peer_id}");
    }
    if tls.is_some() {
        println!("Edge serving https on {http_port}");
    } else {
        println!("Edge serving http on {http_port}");
    }

    canopee_edge::start_edge(network, http_port, tls).await?;
    // The edge runs until killed.
    std::future::pending::<()>().await;
    Ok(())
}