//! The Canopee edge role: a public gateway that turns a publisher's local
//! node into a reachable website.
//!
//! This is a *library* so the role can run in two places:
//!
//! * standalone — the `canopee-edge` binary (see `main.rs`), for a dedicated
//!   gateway host, and
//! * embedded in `canopee-node` — every node (including the public bootstrap
//!   relay) is an edge by default, so `canopee publish` works with zero
//!   edge-side setup.
//!
//! Registering publishers prove ownership of their `username` subdomain: the
//! signed claim's identity must match the owner recorded in the DHT username
//! registry (`username:<name>` → owner). The edge then proxies plain HTTP
//! request/response frames between browsers and publishers over
//! `/canopee/serve/1.0.0` on the existing libp2p connection.

pub mod httpd;
pub mod registry;

use canopee_network::NetworkManager;

pub use registry::Registry;

/// Starts the edge role on an existing swarm: the serve-registry handler
/// (validates publisher registrations) plus the public HTTP(S) front door on
/// `http_port`. Returns the bound port (useful when `http_port == 0`).
pub async fn start_edge(
    network: NetworkManager,
    http_port: u16,
    tls: Option<tokio_rustls::TlsAcceptor>,
) -> anyhow::Result<u16> {
    let registry = Registry::new(network.clone());
    {
        let registry = registry.clone();
        tokio::spawn(async move {
            registry.run().await;
        });
    }

    let httpd = httpd::Httpd::new(network, registry, http_port, tls).await?;
    let bound_port = httpd.local_port();
    tokio::spawn(async move {
        if let Err(e) = httpd.run().await {
            tracing::error!("edge HTTP server stopped: {e}");
        }
    });
    Ok(bound_port)
}
