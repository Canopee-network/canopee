//! The publisher-side serve session: registers this node's connection with a
//! Canopee edge (so the edge routes `<app-hash>.domain/...` traffic here),
//! answers forwarded HTTP requests from the pinned files of the identity's
//! published app, and keeps the registration fresh with a heartbeat.

use crate::http::render_response;
use crate::Runtime;
use canopee_network::{
    InboundServe, Multiaddr, PeerId, ServeRegistration, ServeRegistrationResponse, ServeRequest,
    ServeResponse, app_subdomain, peer_id_from_multiaddr,
};
use canopee_storage::{AppManifest, ObjectId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

/// A publisher re-registers this often. Fast enough to ride out an edge's
/// short-term misses (a dropped `put_record` republishes on the next beat),
/// slow enough not to spam the registry, and well under the swarm's
/// 60s idle-connection timeout so the session link stays warm.
const REGISTER_INTERVAL: Duration = Duration::from_secs(30);
/// How long `start_serve_session` waits for the edge connection to come up
/// before giving up (the registration protocol needs an established link).
const EDGE_DIAL_TIMEOUT: Duration = Duration::from_secs(20);
/// A deregistration is a registration with this timestamp (signed like any
/// other, so only the genuine identity can revoke its own mapping).
pub const DEREGISTER_TIMESTAMP: u64 = 0;

/// A published app pinned for serving: app-relative path → file bytes
/// (`/` is the entrypoint).
pub(crate) type AppFiles = HashMap<String, Vec<u8>>;

/// Handle to a running publish session. Drain it (via
/// `Runtime::stop_serve_session`) to deregister with the edge and stop the
/// heartbeat; dropping the handle without stopping would leave the edge
/// serving requests until the registration TTL expires, so the runtime always
/// owns the session via its `serve` slot.
pub struct ServeSession {
    pub(crate) shutdown: broadcast::Sender<()>,
    pub(crate) running: Arc<AtomicBool>,
    pub(crate) tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    pub(crate) edge_peer_id: PeerId,
    /// The manifest id of the app this session serves (the app is keyed by
    /// content hash, so a new publish of changed content is a *new* app).
    pub(crate) app_id: String,
}

impl ServeSession {
    /// Stops the session's tasks by dropping the shutdown signal, then awaits
    /// them so no handler keeps running past the call.
    pub(crate) async fn terminate(&self) {
        drop(self.shutdown.clone());
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
        for task in tasks {
            if tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .is_err()
            {
                tracing::warn!("serve session task did not stop in time");
            }
        }
        self.running.store(false, Ordering::Relaxed);
    }

    /// The public URL root this session answers:
    /// `https://<app-subdomain>.<base>/`.
    pub fn root_url(&self, public_base_domain: &str) -> String {
        format!(
            "https://{}.{}/",
            app_subdomain(&self.app_id),
            public_base_domain
        )
    }
}

impl Runtime {
    /// Starts a serve session for the published app whose manifest is
    /// `app_id`, via `edge_addr` (a dialable multiaddr ending in
    /// `/p2p/<edge-peer-id>`).
    ///
    /// The edge verifies the signed registration by fetching the manifest
    /// from us and checking its owner is this identity — the app id is a
    /// hash of the manifest, so only the genuine owner can register it. No
    /// username is needed. Replaces any previous session (deregistering it
    /// first).
    ///
    /// Returns the running session and the app id.
    pub async fn start_serve_session(
        &self,
        edge_addr: &str,
        app_id: &ObjectId,
    ) -> anyhow::Result<(Arc<ServeSession>, String)> {
        if self.serve.read().await.is_some() {
            self.stop_serve_session().await;
        }

        // Resolve and pin the app up front: a typo'd or unservable app id
        // fails here, not after the edge has pinned the subdomain.
        let files = Arc::new(self.load_app_files(app_id).await?);

        let edge_addr: Multiaddr = edge_addr.parse().map_err(|e| {
            anyhow::anyhow!("invalid edge address `{edge_addr}`: {e}")
        })?;
        let edge_peer_id = peer_id_from_multiaddr(&edge_addr).ok_or_else(|| {
            anyhow::anyhow!("edge address must end with /p2p/<peer-id>: {edge_addr}")
        })?;
        self.network.dial(edge_addr.clone()).await?;

        // Dialing is asynchronous; the serve-registry protocol needs an
        // established connection (or at least a known address) before the
        // registration request can be sent. Wait for the peer to show up,
        // mirroring the pairing flow's dial-then-wait.
        let deadline = Instant::now() + EDGE_DIAL_TIMEOUT;
        loop {
            if let Ok(peers) = self.network.peers().await {
                if peers.iter().any(|p| p.peer_id == edge_peer_id) {
                    break;
                }
            }
            if Instant::now() >= deadline {
                anyhow::bail!("could not connect to the edge at {edge_addr}");
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let app_id_string = app_id.to_string();
        let registration = ServeRegistration::sign(&self.identity, &app_id_string, now)?;
        match self
            .network
            .send_serve_registration(edge_peer_id, registration)
            .await?
        {
            ServeRegistrationResponse::Ok => {}
            ServeRegistrationResponse::Error(e) => {
                anyhow::bail!("edge refused serving {app_id_string}: {e}")
            }
        }

        let running = Arc::new(AtomicBool::new(true));
        let (shutdown, _) = broadcast::channel(1);

        let runtime = self.clone();
        let files_for_handler = files.clone();
        let running_for_handler = running.clone();
        let mut shutdown_rx = shutdown.subscribe();
        let handler = tokio::spawn(async move {
            let mut events = runtime.network.serve_events();
            loop {
                tokio::select! {
                    inbound = events.recv() => match inbound {
                        Ok(InboundServe { request, reply, .. }) => {
                            let response = runtime
                                .serve_request(&files_for_handler, request)
                                .await;
                            let _ = reply.send(response);
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                    _ = shutdown_rx.recv() => break,
                }
            }
            running_for_handler.store(false, Ordering::Relaxed);
        });

        let runtime = self.clone();
        let shutdown_rx = shutdown.subscribe();
        let heartbeat_runtime = runtime.clone();
        let heartbeat_app_id = app_id_string.clone();
        let heartbeat_running = running.clone();
        let heartbeat = tokio::spawn(async move {
            heartbeat_loop(
                heartbeat_runtime,
                heartbeat_app_id,
                edge_peer_id,
                heartbeat_running,
                shutdown_rx,
            )
            .await;
        });

        let session = Arc::new(ServeSession {
            shutdown,
            running,
            tasks: Mutex::new(vec![handler, heartbeat]),
            edge_peer_id,
            app_id: app_id_string.clone(),
        });
        {
            let mut slot = self.serve.write().await;
            *slot = Some(session.clone());
        }

        Ok((session, app_id_string))
    }

    /// Deregisters the running session (if any) and stops its tasks.
    pub async fn stop_serve_session(&self) {
        let Some(session) = self.serve.write().await.take() else {
            return;
        };
        // Signed deregistration so the edge drops the app↔peer mapping.
        let dereg = ServeRegistration::sign(
            &self.identity,
            &session.app_id,
            DEREGISTER_TIMESTAMP,
        )
        .ok();
        if let Some(dereg) = dereg {
            if let Ok(ServeRegistrationResponse::Ok) = self
                .network
                .send_serve_registration(session.edge_peer_id, dereg)
                .await
            {
                tracing::info!("deregistered {} from edge", session.app_id);
            } else {
                tracing::warn!(
                    "edge deregistration for {} failed; its registry entry will expire",
                    session.app_id
                );
            }
        }
        session.terminate().await;
    }

    /// Serves one forwarded HTTP request against the session's pinned files.
    /// The path is app-relative: `/` is the entrypoint, anything else an
    /// asset path (with the SPA fallback handled by `render_response`).
    async fn serve_request(
        &self,
        files: &Arc<AppFiles>,
        request: ServeRequest,
    ) -> ServeResponse {
        let mut path: &str = &request.path;
        if let Some(no_query) = path.split('?').next() {
            if !no_query.is_empty() {
                path = no_query;
            }
        }
        if !path.starts_with('/') {
            path = "/";
        }
        render_response(files, &request.method, path, &request.headers)
    }

    /// Resolves the file map for the app whose manifest is `app_id`. Only
    /// this identity's own, explicitly shared (announced) apps are servable —
    /// anything else fails before the edge pins the subdomain.
    async fn load_app_files(&self, app_id: &ObjectId) -> anyhow::Result<AppFiles> {
        let manifest_object = self
            .storage
            .get_verified(app_id)
            .await
            .map_err(|e| anyhow::anyhow!("app manifest {app_id} is not stored here: {e}"))?;
        if manifest_object.payload.owner != *self.identity.id() {
            anyhow::bail!("app manifest {app_id} belongs to another identity");
        }
        if !self.shared.contains(app_id).await {
            anyhow::bail!(
                "app manifest {app_id} is not shared; publish it first so the edge can verify it"
            );
        }
        let manifest: AppManifest = manifest_object.decode()?;

        let entrypoint = self
            .storage
            .get_verified(&manifest.entrypoint)
            .await
            .map_err(|e| anyhow::anyhow!("entrypoint for app {app_id} missing: {e}"))?;
        let mut files: AppFiles = HashMap::new();
        files.insert("/".to_string(), entrypoint.payload.data);
        for (path, asset_id) in &manifest.assets {
            match self.storage.get_verified(asset_id).await {
                Ok(asset) => {
                    files.insert(path.clone(), asset.payload.data);
                }
                Err(e) => {
                    tracing::warn!("asset {path} of app {app_id} missing, skipping: {e}");
                }
            }
        }
        Ok(files)
    }
}

/// Re-registers the published session every `REGISTER_INTERVAL` so the edge
/// keeps the app pinned to this connection and the connection stays warm
/// (below the swarm idle timeout).
async fn heartbeat_loop(
    runtime: Runtime,
    app_id: String,
    edge_peer_id: PeerId,
    running: Arc<AtomicBool>,
    mut shutdown: broadcast::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(REGISTER_INTERVAL) => {}
            _ = shutdown.recv() => break,
        }
        if !running.load(Ordering::Relaxed) {
            break;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let Ok(registration) = ServeRegistration::sign(&runtime.identity, &app_id, now) else {
            continue;
        };
        match runtime
            .network
            .send_serve_registration(edge_peer_id, registration)
            .await
        {
            Ok(ServeRegistrationResponse::Ok) => {}
            Ok(ServeRegistrationResponse::Error(e)) => {
                tracing::warn!("edge re-registration for {app_id} refused: {e}");
            }
            Err(e) => {
                tracing::warn!("edge re-registration for {app_id} failed: {e}");
            }
        }
    }
}