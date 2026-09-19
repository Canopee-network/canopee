use crate::behaviour::{
    CanopeeBehaviour, IDENTIFY_PROTOCOL, KAD_PROTOCOL, PAIRING_PROTOCOL, SERVE_PROTOCOL,
    SERVE_REGISTRY_PROTOCOL,
};
use crate::message::{
    CanopeePairingRequest, CanopeePairingResponse, ObjectRequest, ObjectResponse, PubSubMessage,
    ServeRegistration, ServeRegistrationResponse, ServeRequest, ServeResponse,
};
use crate::manager::event_loop::run_event_loop;
use crate::peer::{Peer, RelayReservation};
use canopee_identity::IdentityId;
use canopee_storage::{ExportBundle, ObjectId};

use libp2p::kad::{self, store::MemoryStore};
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::{
    Multiaddr, PeerId, StreamProtocol, SwarmBuilder, autonat, dcutr, gossipsub, identify, mdns,
    noise, ping, relay, tcp, yamux,
};
use libp2p::identity::Keypair;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};

/// Extracts the relay's peer id from a `/p2p-circuit` listen address, i.e. one
/// of the form `.../p2p/<relay-id>/p2p-circuit(/p2p/<our-id>)?`.
fn relay_peer_id_from_circuit_addr(addr: &Multiaddr) -> Option<PeerId> {
    let mut last_p2p = None;
    for protocol in addr.iter() {
        match protocol {
            Protocol::P2p(peer_id) => last_p2p = Some(peer_id),
            Protocol::P2pCircuit => return last_p2p,
            _ => {}
        }
    }
    None
}

/// Default bootstrap relay addresses. New nodes dial these on startup to get
/// their first Kademlia routing-table entries, after which normal DHT
/// discovery takes over. The default relay also runs the edge role, so this
/// is the default edge for `canopee publish`.
pub const DEFAULT_BOOTSTRAP_ADDRS: &[&str] =
    &["/ip4/89.127.234.35/tcp/4001/p2p/12D3KooWGiPk75fg8HBW7WJCouTTTLNi8W3s48sBK8AKewZKbCjC"];

/// Returns the list of bootstrap multiaddrs to dial at startup.
///
/// If `CANOPEE_BOOTSTRAP_ADDRS` is set, its comma-separated values are used
/// *instead of* the defaults. If `CANOPEE_BOOTSTRAP_ADDRS_PREPEND` is set to
/// `1`, the env-var values are *prepended* to the defaults rather than
/// replacing them.
fn bootstrap_addrs() -> Vec<Multiaddr> {
    let env_val = std::env::var("CANOPEE_BOOTSTRAP_ADDRS").ok();
    let prepend = std::env::var("CANOPEE_BOOTSTRAP_ADDRS_PREPEND")
        .map(|v| v == "1")
        .unwrap_or(false);

    let env_addrs: Vec<Multiaddr> = env_val
        .as_deref()
        .map(|v| {
            v.split(',')
                .filter_map(|s| {
                    let s = s.trim();
                    if s.is_empty() {
                        return None;
                    }
                    match s.parse::<Multiaddr>() {
                        Ok(a) => Some(a),
                        Err(e) => {
                            tracing::warn!("Ignoring invalid bootstrap addr '{s}': {e}");
                            None
                        }
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let default_addrs: Vec<Multiaddr> = DEFAULT_BOOTSTRAP_ADDRS
        .iter()
        .filter_map(|s| {
            s.parse::<Multiaddr>()
                .map_err(|e| {
                    tracing::error!("Invalid default bootstrap addr '{s}': {e}");
                    e
                })
                .ok()
        })
        .collect();

    if prepend {
        let mut addrs = env_addrs;
        addrs.extend(default_addrs);
        addrs
    } else if env_val.is_some() {
        env_addrs
    } else {
        default_addrs
    }
}

#[async_trait::async_trait]
pub trait ObjectProvider: Send + Sync + 'static {
    async fn get_object(&self, id: &ObjectId) -> Option<ExportBundle>;
}

enum Command {
    Dial(Multiaddr),
    ListenViaRelay(Multiaddr),
    FindProviders {
        object_id: ObjectId,
        reply: oneshot::Sender<Vec<PeerId>>,
    },
    Announce(ObjectId),
    Unannounce(ObjectId),
    PutRecord {
        key: Vec<u8>,
        value: Vec<u8>,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    GetRecord {
        key: Vec<u8>,
        reply: oneshot::Sender<anyhow::Result<Option<Vec<u8>>>>,
    },
    GetObject {
        peer_id: PeerId,
        object_id: ObjectId,
        reply: oneshot::Sender<anyhow::Result<ExportBundle>>,
    },
    /// Sends a LAN pairing request to `peer_id` over `/canopee/pairing/1.0.0`
    /// and awaits the response.
    SendPairing {
        peer_id: PeerId,
        request: CanopeePairingRequest,
        reply: oneshot::Sender<anyhow::Result<String>>,
    },
    /// Registers this node (plus the connection the edge dials back) as the
    /// server for the signed `registration`'s username. The reply carries the
    /// edge's acknowledgement or rejection.
    SendServeRegistration {
        edge_peer_id: PeerId,
        registration: ServeRegistration,
        reply: oneshot::Sender<anyhow::Result<ServeRegistrationResponse>>,
    },
    /// Forwards one HTTP request to `peer_id` (a publisher that registered
    /// with us) and awaits its [`ServeResponse`].
    SendServeRequest {
        peer_id: PeerId,
        request: ServeRequest,
        reply: oneshot::Sender<anyhow::Result<ServeResponse>>,
    },
    ListPeers(oneshot::Sender<Vec<Peer>>),
    /// Updates the tracked metadata (identity / username / display name) for
    /// a connected peer, resolved out-of-band (e.g. from the peer's signed
    /// profile and username records).
    SetPeerMeta {
        peer_id: PeerId,
        identity: Option<IdentityId>,
        username: Option<String>,
        display_name: Option<String>,
    },
    ListListenAddresses(oneshot::Sender<Vec<Multiaddr>>),
    ListRelayReservations(oneshot::Sender<Vec<RelayReservation>>),
    Subscribe(String, oneshot::Sender<anyhow::Result<()>>),
    Unsubscribe(String),
    Publish {
        topic: String,
        data: Vec<u8>,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
}

/// An inbound LAN pairing request, forwarded to the runtime (which subscribes
/// via [`NetworkManager::pairing_events`]) for decryption + import. The
/// `reply` sender lets the runtime answer the other device; the manager holds
/// the response channel until a reply arrives.
#[derive(Debug, Clone)]
pub struct InboundPairing {
    pub peer: PeerId,
    pub request: CanopeePairingRequest,
    pub reply: mpsc::UnboundedSender<CanopeePairingResponse>,
}

/// An inbound HTTP request from an edge, forwarded to the publisher's serve
/// session (which subscribes via [`NetworkManager::serve_events`]). The
/// publisher must answer with a [`ServeResponse`] on the unbounded `reply`;
/// if the publisher's session produces no answer, a 500-style default is sent.
#[derive(Clone)]
pub struct InboundServe {
    pub peer: PeerId,
    pub request: ServeRequest,
    pub reply: mpsc::UnboundedSender<ServeResponse>,
}

/// An inbound registration claim from a publisher, forwarded to the *edge*'s
/// registry handler (which subscribes via [`NetworkManager::serve_registry_events`]).
#[derive(Clone)]
pub struct InboundServeRegistration {
    pub peer: PeerId,
    pub request: ServeRegistration,
    pub reply: mpsc::UnboundedSender<ServeRegistrationResponse>,
}

#[derive(Clone)]
pub struct NetworkManager {
    commands: mpsc::Sender<Command>,
    pubsub: broadcast::Sender<PubSubMessage>,
    pairing_events: broadcast::Sender<InboundPairing>,
    serve_events: broadcast::Sender<InboundServe>,
    serve_registry_events: broadcast::Sender<InboundServeRegistration>,
}

impl NetworkManager {
    /// Builds a swarm for `device_key` — the node's *device* keypair, whose
    /// public key is the machine's network identity. The person identity key
    /// is deliberately not used for networking: device keys let several
    /// devices of one identity be online at once, each with its own `PeerId`
    /// (see the `(owner, "devices")` record and the `device:<peer-id>`
    /// registry in canopee-storage for the reverse mapping).
    /// `listen_addr` is where the swarm binds (use `/ip4/0.0.0.0/tcp/0` for an
    /// OS-assigned port); `object_provider` serves this node's stored objects
    /// to other peers.
    ///
    /// `mdns` controls multicast discovery. Keep it on for a standalone node;
    /// embedded apps that share one identity with other apps should pass
    /// `false` so a second app's swarm never re-announces the same `PeerId`
    /// over mDNS (they still find each other via Kademlia/bootstrap + dialing).
    pub fn new(
        device_key: Keypair,
        listen_addr: Multiaddr,
        object_provider: Arc<dyn ObjectProvider>,
        mdns: bool,
    ) -> anyhow::Result<Self> {
        // create peer_id from the device key...
        let peer_id = PeerId::from(device_key.public());

        // build Swarm with relay client and behabviour
        let mut swarm = SwarmBuilder::with_existing_identity(device_key)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_relay_client(noise::Config::new, yamux::Config::default)?
            .with_behaviour(|key, relay_client| {
                let identify = identify::Behaviour::new(identify::Config::new(
                    IDENTIFY_PROTOCOL.to_string(),
                    key.public(),
                ));

                let kad_protocol = StreamProtocol::try_from_owned(
                    String::from_utf8(KAD_PROTOCOL.to_vec()).unwrap(),
                )
                .map_err(|e| Box::<dyn std::error::Error + Send + Sync>::from(e.to_string()))?;
                let kad_config = kad::Config::new(kad_protocol);
                let mut kad =
                    kad::Behaviour::with_config(peer_id, MemoryStore::new(peer_id), kad_config);
                kad.set_mode(Some(kad::Mode::Server));

                let object_exchange =
                    request_response::cbor::Behaviour::<ObjectRequest, ObjectResponse>::new(
                        [(
                            StreamProtocol::new("/canopee/objects/1.0.0"),
                            ProtocolSupport::Full,
                        )],
                        request_response::Config::default(),
                    );

                let pairing =
                    request_response::cbor::Behaviour::<CanopeePairingRequest, CanopeePairingResponse>::new(
                        [(StreamProtocol::new(PAIRING_PROTOCOL), ProtocolSupport::Full)],
                        request_response::Config::default(),
                    );

                let serve =
                    request_response::cbor::Behaviour::<ServeRequest, ServeResponse>::new(
                        [(StreamProtocol::new(SERVE_PROTOCOL), ProtocolSupport::Full)],
                        request_response::Config::default(),
                    );

                let serve_registry = request_response::cbor::Behaviour::<
                    ServeRegistration,
                    ServeRegistrationResponse,
                >::new(
                    [(StreamProtocol::new(
                        SERVE_REGISTRY_PROTOCOL,
                    ), ProtocolSupport::Full)],
                    request_response::Config::default(),
                );

                let ping = ping::Behaviour::new(ping::Config::default());

                let mdns = if mdns {
                    Some(
                        mdns::tokio::Behaviour::new(mdns::Config::default(), peer_id).map_err(
                            |e| Box::<dyn std::error::Error + Send + Sync>::from(e.to_string()),
                        )?,
                    )
                } else {
                    None
                };
                let mdns = libp2p::swarm::behaviour::toggle::Toggle::from(mdns);

                let gossipsub = gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(key.clone()),
                    gossipsub::Config::default(),
                )
                .map_err(|e| Box::<dyn std::error::Error + Send + Sync>::from(e.to_string()))?;

                let relay = relay::Behaviour::new(peer_id, relay::Config::default());
                let dcutr = dcutr::Behaviour::new(peer_id);
                let autonat = autonat::Behaviour::new(peer_id, autonat::Config::default());

                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(CanopeeBehaviour {
                    identify,
                    kad,
                    object_exchange,
                    pairing,
                    serve,
                    serve_registry,
                    ping,
                    mdns,
                    gossipsub,
                    relay,
                    relay_client,
                    dcutr,
                    autonat,
                })
            })?
            .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        // smarm listener...
        swarm.listen_on(listen_addr)?;

        // dial bootstrap nodes so a fresh node gets its first routing-table entries
        let bootstrap = bootstrap_addrs();
        if !bootstrap.is_empty() {
            tracing::info!("Dialing {} bootstrap address(es)", bootstrap.len());
        }
        for addr in bootstrap {
            if let Err(e) = swarm.dial(addr.clone()) {
                tracing::warn!("Failed to dial bootstrap {addr}: {e}");
            }
        }

        // start listener loop and spawn processes...
        let (tx, rx) = mpsc::channel(64);
        let (pubsub_tx, _) = broadcast::channel(256);
        let (pairing_tx, _) = broadcast::channel(64);
        let (serve_tx, _) = broadcast::channel(64);
        let (serve_registry_tx, _) = broadcast::channel(64);
        tokio::spawn(run_event_loop(
            swarm,
            rx,
            object_provider,
            pubsub_tx.clone(),
            pairing_tx.clone(),
            serve_tx.clone(),
            serve_registry_tx.clone(),
        ));

        Ok(Self {
            commands: tx,
            pubsub: pubsub_tx,
            pairing_events: pairing_tx,
            serve_events: serve_tx,
            serve_registry_events: serve_registry_tx,
        })
    }

    pub async fn dial(&self, addr: Multiaddr) -> anyhow::Result<()> {
        self.commands.send(Command::Dial(addr)).await?;
        Ok(())
    }

    /// Requests a circuit reservation on `relay_addr` (a full address including
    /// the relay's `/p2p/<peer_id>` suffix) so peers behind other NATs can reach
    /// us via that relay, and dcutr can then attempt to upgrade to a direct connection.
    pub async fn listen_via_relay(&self, relay_addr: Multiaddr) -> anyhow::Result<()> {
        self.commands
            .send(Command::ListenViaRelay(relay_addr))
            .await?;
        Ok(())
    }

    pub async fn find_providers(&self, object_id: ObjectId) -> anyhow::Result<Vec<PeerId>> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::FindProviders { object_id, reply })
            .await?;
        Ok(rx.await?)
    }

    pub async fn announce(&self, object_id: ObjectId) -> anyhow::Result<()> {
        self.commands.send(Command::Announce(object_id)).await?;
        Ok(())
    }

    /// Withdraws this node as a provider of `object_id` on the DHT (used when
    /// a cached object is evicted, so `find_providers` stops listing us as a
    /// provider for something we no longer hold). Best-effort: if the node
    /// is not currently connected to the DHT this is a no-op warning.
    pub async fn unannounce(&self, object_id: ObjectId) -> anyhow::Result<()> {
        self.commands.send(Command::Unannounce(object_id)).await?;
        Ok(())
    }

    /// Returns a broadcast receiver for inbound LAN pairing requests. Exactly
    /// one subscriber (the runtime's pairing handler) should consume these;
    /// each `InboundPairing` carries an unbounded reply channel that must be
    /// answered for the remote device's `send_pairing` call to return.
    pub fn pairing_events(&self) -> broadcast::Receiver<InboundPairing> {
        self.pairing_events.subscribe()
    }

    /// Sends a LAN pairing request to `peer_id` over `/canopee/pairing/1.0.0`
    /// and awaits the remote's answer. Returns the remote's `Accepted` status
    /// message, or an error on transport failure / refusal.
    pub async fn send_pairing(
        &self,
        peer_id: PeerId,
        request: CanopeePairingRequest,
    ) -> anyhow::Result<String> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::SendPairing {
                peer_id,
                request,
                reply,
            })
            .await?;
        rx.await?
    }

    /// Registers a signed username/connection claim with an edge. The edge
    /// verifies the signature and reverses the username to the same identity
    /// via the DHT, then pins the subdomain to *this* connection (the
    /// connection the edge dialed to reach us) so it knows which peer to
    /// forward `identity.domain` HTTP traffic to.
    pub async fn send_serve_registration(
        &self,
        edge_peer_id: PeerId,
        registration: ServeRegistration,
    ) -> anyhow::Result<ServeRegistrationResponse> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::SendServeRegistration {
                edge_peer_id,
                registration,
                reply,
            })
            .await?;
        rx.await?
    }

    /// Forwards a single HTTP request to `peer_id` and awaits the serving
    /// publisher's [`ServeResponse`]. Only edges should call this (after a
    /// successful registry lookup); a publisher answering `serve_events`
    /// satisfies it.
    pub async fn send_serve_request(
        &self,
        peer_id: PeerId,
        request: ServeRequest,
    ) -> anyhow::Result<ServeResponse> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::SendServeRequest {
                peer_id,
                request,
                reply,
            })
            .await?;
        rx.await?
    }

    /// Returns a broadcast receiver for inbound HTTP requests forwarded by an
    /// edge. Exactly one subscriber (the runtime's serve session) should
    /// consume these; each `InboundServe` carries a reply channel that must be
    /// answered with a [`ServeResponse`].
    pub fn serve_events(&self) -> broadcast::Receiver<InboundServe> {
        self.serve_events.subscribe()
    }

    /// Returns a broadcast receiver for inbound registration claims from
    /// publishers. Exactly one subscriber (the edge's registry handler) should
    /// consume these.
    pub fn serve_registry_events(&self) -> broadcast::Receiver<InboundServeRegistration> {
        self.serve_registry_events.subscribe()
    }

    /// Publishes an arbitrary, mutable DHT record under `key` (unlike
    /// `announce`, which just marks this node as a provider of an existing
    /// content-addressed object). Overwrites whatever was previously stored
    /// at `key`, network-wide — used for app pointers so republishing under
    /// the same name updates what fetchers resolve to.
    pub async fn put_record(&self, key: Vec<u8>, value: Vec<u8>) -> anyhow::Result<()> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::PutRecord { key, value, reply })
            .await?;
        rx.await?
    }

    /// Looks up a record previously published with `put_record`. Returns
    /// `None` if no record is found for `key`.
    pub async fn get_record(&self, key: Vec<u8>) -> anyhow::Result<Option<Vec<u8>>> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::GetRecord { key, reply })
            .await?;
        rx.await?
    }

    pub async fn get_object(
        &self,
        peer_id: PeerId,
        object_id: ObjectId,
    ) -> anyhow::Result<ExportBundle> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::GetObject {
                peer_id,
                object_id,
                reply,
            })
            .await?;
        rx.await?
    }

    pub async fn peers(&self) -> anyhow::Result<Vec<Peer>> {
        let (reply, rx) = oneshot::channel();
        self.commands.send(Command::ListPeers(reply)).await?;
        Ok(rx.await?)
    }

    /// Best-effort: enriches the tracked metadata for a connected peer (its
    /// canonical identity, and any resolved username/display name). No-op if
    /// the peer is no longer connected.
    pub async fn set_peer_meta(
        &self,
        peer_id: PeerId,
        identity: Option<IdentityId>,
        username: Option<String>,
        display_name: Option<String>,
    ) -> anyhow::Result<()> {
        self.commands
            .send(Command::SetPeerMeta {
                peer_id,
                identity,
                username,
                display_name,
            })
            .await?;
        Ok(())
    }

    /// Returns the addresses this node is actually bound to (one per
    /// interface). Useful with an ephemeral listen port (e.g.
    /// `/ip4/127.0.0.1/tcp/0`) to learn the real port after startup.
    pub async fn listen_addresses(&self) -> anyhow::Result<Vec<Multiaddr>> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::ListListenAddresses(reply))
            .await?;
        Ok(rx.await?)
    }

    /// Lists currently accepted relay circuit reservations, including the
    /// dialable addresses learned for each. Use this to confirm a
    /// `listen_via_relay` request actually succeeded before telling other
    /// peers to dial you through that relay.
    pub async fn relay_reservations(&self) -> anyhow::Result<Vec<RelayReservation>> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::ListRelayReservations(reply))
            .await?;
        Ok(rx.await?)
    }

    /// Subscribes to a gossipsub topic and returns a receiver for messages on
    /// any subscribed topic. Filter on `PubSubMessage::topic` if subscribed to more than one.
    pub async fn subscribe(
        &self,
        topic: &str,
    ) -> anyhow::Result<broadcast::Receiver<PubSubMessage>> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::Subscribe(topic.to_string(), reply))
            .await?;
        rx.await??;
        Ok(self.pubsub.subscribe())
    }

    pub async fn unsubscribe(&self, topic: &str) -> anyhow::Result<()> {
        self.commands
            .send(Command::Unsubscribe(topic.to_string()))
            .await?;
        Ok(())
    }

    pub async fn publish(&self, topic: &str, data: Vec<u8>) -> anyhow::Result<()> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::Publish {
                topic: topic.to_string(),
                data,
                reply,
            })
            .await?;
        rx.await?
    }
}

mod event_loop;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    /// The env vars `bootstrap_addrs()` reads are process-global, so tests
    /// that set them must be serialized against each other and against any
    /// test that spawns a real `NetworkManager` (which reads them too).
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static SETUP: Once = Once::new();

    fn clear_bootstrap_env() {
        SETUP.call_once(|| {
            // Additionally save any pre-existing values so `cargo test` inside a
            // developer shell that already exports these vars fails loudly
            // rather than silently producing a degraded list.
            if std::env::var_os("CANOPEE_BOOTSTRAP_ADDRS").is_some()
                || std::env::var_os("CANOPEE_BOOTSTRAP_ADDRS_PREPEND").is_some()
            {
                panic!(
                    "CANOPEE_BOOTSTRAP_ADDRS(_PREPEND) is set in the test \
                     environment; unset it before running these tests so they \
                     test the defaults deterministically"
                );
            }
        });
    }

    /// Sets an env var for the duration of the closure, restoring it after.
    fn with_env<K: AsRef<str>, V: AsRef<str>>(
        key: K,
        value: Option<V>,
        f: impl FnOnce(),
    ) {
        let key = key.as_ref();
        let prev = std::env::var_os(key);
        match value {
            Some(v) => unsafe {
                std::env::set_var(key, v.as_ref());
            },
            None => unsafe {
                std::env::remove_var(key);
            },
        }
        f();
        match prev {
            Some(v) => unsafe {
                std::env::set_var(key, v);
            },
            None => unsafe {
                std::env::remove_var(key);
            },
        }
    }

    fn addrs() -> Vec<String> {
        bootstrap_addrs().into_iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn defaults_used_when_no_env_override() {
        let _guard = LOCK.lock().unwrap();
        clear_bootstrap_env();
        assert!(!addrs().is_empty(), "default bootstrap list must not be empty");
        assert_eq!(addrs()[0].parse::<Multiaddr>().unwrap().to_string(), addrs()[0]);
    }

    #[test]
    fn env_override_replaces_defaults() {
        let _guard = LOCK.lock().unwrap();
        clear_bootstrap_env();
        with_env("CANOPEE_BOOTSTRAP_ADDRS", Some("/ip4/127.0.0.1/tcp/9999/p2p/12D3KooWGiPk75fg8HBW7WJCouTTTLNi8W3s48sBK8AKewZKbCjC"), || {
            let addrs = addrs();
            assert_eq!(addrs.len(), 1, "override must replace the defaults entirely");
            assert!(addrs[0].starts_with("/ip4/127.0.0.1/tcp/9999"));
        });
    }

    #[test]
    fn prepend_flag_extends_defaults_with_env() {
        let _guard = LOCK.lock().unwrap();
        clear_bootstrap_env();
        with_env(
            "CANOPEE_BOOTSTRAP_ADDRS",
            Some("/ip4/127.0.0.1/tcp/9999/p2p/12D3KooWGiPk75fg8HBW7WJCouTTTLNi8W3s48sBK8AKewZKbCjC"),
            || {
                with_env("CANOPEE_BOOTSTRAP_ADDRS_PREPEND", Some("1"), || {
                    let addrs = addrs();
                    assert!(addrs.len() > 1, "prepend keeps the defaults too");
                    assert!(addrs[0].starts_with("/ip4/127.0.0.1/tcp/9999"));
                });
            },
        );
    }

    #[test]
    fn empty_override_opts_out_of_defaults() {
        let _guard = LOCK.lock().unwrap();
        clear_bootstrap_env();
        with_env("CANOPEE_BOOTSTRAP_ADDRS", Some(""), || {
            assert!(
                addrs().is_empty(),
                "an explicit empty override means 'dial nothing', not 'use defaults'"
            );
        });
    }

    #[test]
    fn invalid_entries_are_dropped_not_fatal() {
        let _guard = LOCK.lock().unwrap();
        clear_bootstrap_env();
        with_env(
            "CANOPEE_BOOTSTRAP_ADDRS",
            Some("/ip4/127.0.0.1/tcp/9999/p2p/12D3KooWGiPk75fg8HBW7WJCouTTTLNi8W3s48sBK8AKewZKbCjC,not-a-multiaddr,,  "),
            || {
                let addrs = addrs();
                assert_eq!(addrs.len(), 1, "garbage and blanks must be skipped");
                assert!(addrs[0].starts_with("/ip4/127.0.0.1/tcp/9999"));
            },
        );
    }
}

