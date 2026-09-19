//! The edge's registry: keeps `app subdomain -> connected publisher peer`
//! while the publisher's signed registrations stay fresh, and answers the
//! inbound `/canopee/serve-registry/1.0.0` requests by validating them.
//!
//! An app is keyed by its manifest hash, so validation needs no namespace:
//! the edge fetches the manifest from the registering peer and checks its
//! owner is the identity that signed the claim. Only the owner can produce
//! that pair, so nobody can register (or hijack) someone else's app.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use canopee_network::{
    InboundServeRegistration, NetworkManager, PeerId, ServeRegistration, ServeRegistrationResponse,
    app_subdomain,
};
use canopee_storage::{AppManifest, ObjectId};
use tokio::sync::{Mutex, broadcast};

/// How long a registration stays valid without a heartbeat. The publisher
/// re-registers every 30s; 3 missed beats force a re-registration before a
/// stale (e.g. crashed or offline) node is served.
const REGISTRY_TTL: Duration = Duration::from_secs(90);
/// Maximum clock skew the edge tolerates between its own clock and the
/// publisher's registration timestamp.
const MAX_TIMESTAMP_SKEW_SECS: u64 = 120;
/// Serve requests that a publisher does not answer within this window are
/// failed as a gateway error rather than held open forever.
const FORWARD_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct Entry {
    peer: PeerId,
    /// The identity whose ownership of the app was validated at registration
    /// — heartbeats from the same identity skip the manifest re-fetch.
    identity: String,
    last_seen: Instant,
}

/// App subdomain (first 32 hex chars of the manifest id) → the publisher
/// connection currently serving it. The map is behind its own mutex so the
/// event loop never holds a lock across an await (a manifest fetch must not
/// block HTTP forwarding).
#[derive(Clone)]
pub struct Registry {
    network: NetworkManager,
    entries: Arc<Mutex<HashMap<String, Entry>>>,
}

impl Registry {
    pub fn new(network: NetworkManager) -> Self {
        Self {
            network,
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The publisher currently serving `subdomain`, or `None` if it has not
    /// registered (or its registration expired).
    pub async fn resolve(&self, subdomain: &str) -> Option<PeerId> {
        let subdomain = subdomain.to_ascii_lowercase();
        let now = Instant::now();
        match self.entries.lock().await.get(&subdomain) {
            Some(entry) if now.duration_since(entry.last_seen) <= REGISTRY_TTL => {
                Some(entry.peer)
            }
            _ => None,
        }
    }

    async fn handle(&self, event: InboundServeRegistration) {
        let InboundServeRegistration {
            peer,
            request,
            reply,
        } = event;

        let subdomain = app_subdomain(request.app_id.trim()).to_string();

        // A signed timestamp-0 registration is a deregistration: drop the
        // mapping (nothing routes to this publisher anymore) and acknowledge.
        if request.timestamp == 0 {
            tracing::info!("deregistration from {peer} for {subdomain}");
            self.entries.lock().await.remove(&subdomain);
            let _ = reply.send(ServeRegistrationResponse::Ok);
            return;
        }

        // Heartbeat from the already-validated identity: just refresh.
        {
            let entries = self.entries.lock().await;
            if let Some(entry) = entries.get(&subdomain) {
                if entry.peer == peer
                    && entry.identity == request.identity.to_string()
                    && request.verify()
                {
                    let identity = entry.identity.clone();
                    drop(entries);
                    tracing::debug!("heartbeat for {subdomain} from {peer}");
                    self.entries.lock().await.insert(
                        subdomain,
                        Entry {
                            peer,
                            identity,
                            last_seen: Instant::now(),
                        },
                    );
                    let _ = reply.send(ServeRegistrationResponse::Ok);
                    return;
                }
            }
        }

        let verdict = self.validate(peer, &request).await;

        let response = match verdict {
            Ok(()) => {
                tracing::info!("registered {subdomain} -> {peer}");
                self.entries.lock().await.insert(
                    subdomain,
                    Entry {
                        peer,
                        identity: request.identity.to_string(),
                        last_seen: Instant::now(),
                    },
                );
                ServeRegistrationResponse::Ok
            }
            Err(message) => {
                tracing::warn!("registration from {peer} for {subdomain} refused: {message}");
                ServeRegistrationResponse::Error(message)
            }
        };
        let _ = reply.send(response);
    }

    /// Validates a first registration: the signature must verify, the
    /// timestamp must be fresh, and the manifest — fetched from the
    /// registering peer itself — must hash to `app_id` and name the signing
    /// identity as its owner.
    async fn validate(
        &self,
        peer: PeerId,
        registration: &ServeRegistration,
    ) -> Result<(), String> {
        if !registration.verify() {
            return Err("registration signature does not verify".to_string());
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let skew = now.abs_diff(registration.timestamp);
        if skew > MAX_TIMESTAMP_SKEW_SECS {
            return Err(format!(
                "registration timestamp {skew}s from the edge clock is stale"
            ));
        }

        let app_id = ObjectId::new(registration.app_id.trim());
        let bundle = self
            .network
            .get_object(peer, app_id.clone())
            .await
            .map_err(|e| format!("could not fetch the app manifest from you: {e}"))?;
        // Content-addressing check, not trust: the served payload must hash
        // to the claimed app id. The `id` field alone proves nothing — a
        // malicious node could serve a tampered payload under it.
        if ObjectId::from_payload(&bundle.object.payload) != app_id {
            return Err("the manifest you served does not hash to the claimed app id".to_string());
        }
        let manifest: AppManifest = bundle
            .object
            .decode()
            .map_err(|e| format!("the app id is not an app manifest: {e}"))?;
        if manifest.owner != registration.identity {
            return Err(format!(
                "app {} belongs to {}, not {}",
                app_subdomain(&registration.app_id),
                manifest.owner,
                registration.identity
            ));
        }

        Ok(())
    }

    pub async fn run(&self) {
        let mut events: broadcast::Receiver<InboundServeRegistration> =
            self.network.serve_registry_events();
        loop {
            match events.recv().await {
                Ok(event) => self.handle(event).await,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    pub fn forward_timeout(&self) -> Duration {
        FORWARD_TIMEOUT
    }
}