use crate::behaviour::{CanopeeBehaviour, CanopeeBehaviourEvent, IDENTIFY_PROTOCOL, KAD_PROTOCOL};
use crate::message::{ObjectRequest, ObjectResponse, PubSubMessage};
use crate::peer::{Peer, RelayReservation};
use canopee_identity::Identity;
use canopee_storage::{ExportBundle, ObjectId};
use futures::StreamExt;
use libp2p::kad::{self, store::MemoryStore};
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::SwarmEvent;
use libp2p::{
    Multiaddr, PeerId, StreamProtocol, SwarmBuilder, autonat, dcutr, gossipsub, identify, mdns,
    noise, ping, relay, tcp, yamux,
};
use std::collections::HashMap;
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
    GetObject {
        peer_id: PeerId,
        object_id: ObjectId,
        reply: oneshot::Sender<anyhow::Result<ExportBundle>>,
    },
    ListPeers(oneshot::Sender<Vec<Peer>>),
    ListRelayReservations(oneshot::Sender<Vec<RelayReservation>>),
    Subscribe(String, oneshot::Sender<anyhow::Result<()>>),
    Unsubscribe(String),
    Publish {
        topic: String,
        data: Vec<u8>,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
}

#[derive(Clone)]
pub struct NetworkManager {
    commands: mpsc::Sender<Command>,
    pubsub: broadcast::Sender<PubSubMessage>,
}

impl NetworkManager {
    pub fn new(
        identity: Arc<Identity>,
        listen_addr: Multiaddr,
        object_provider: Arc<dyn ObjectProvider>,
    ) -> anyhow::Result<Self> {
        let keypair = identity.keypair();
        let peer_id = PeerId::from(keypair.public());

        let mut swarm = SwarmBuilder::with_existing_identity(keypair)
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

                let object_exchange = request_response::cbor::Behaviour::<
                    ObjectRequest,
                    ObjectResponse,
                >::new(
                    [(
                        StreamProtocol::new("/canopee/objects/1.0.0"),
                        ProtocolSupport::Full,
                    )],
                    request_response::Config::default(),
                );

                let ping = ping::Behaviour::new(ping::Config::default());

                let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), peer_id)
                    .map_err(|e| Box::<dyn std::error::Error + Send + Sync>::from(e.to_string()))?;

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

        swarm.listen_on(listen_addr)?;

        let (tx, rx) = mpsc::channel(64);
        let (pubsub_tx, _) = broadcast::channel(256);
        tokio::spawn(run_event_loop(swarm, rx, object_provider, pubsub_tx.clone()));

        Ok(Self {
            commands: tx,
            pubsub: pubsub_tx,
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
    pub async fn subscribe(&self, topic: &str) -> anyhow::Result<broadcast::Receiver<PubSubMessage>> {
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

async fn run_event_loop(
    mut swarm: libp2p::Swarm<CanopeeBehaviour>,
    mut commands: mpsc::Receiver<Command>,
    object_provider: Arc<dyn ObjectProvider>,
    pubsub: broadcast::Sender<PubSubMessage>,
) {
    let mut peers: HashMap<PeerId, Peer> = HashMap::new();
    let mut relay_reservations: HashMap<PeerId, RelayReservation> = HashMap::new();
    let mut pending_get_providers: HashMap<kad::QueryId, oneshot::Sender<Vec<PeerId>>> =
        HashMap::new();
    let mut pending_get_object: HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<ExportBundle>>,
    > = HashMap::new();

    loop {
        tokio::select! {
            event = swarm.select_next_some() => {
                handle_swarm_event(
                    event,
                    &mut swarm,
                    &mut peers,
                    &mut relay_reservations,
                    &mut pending_get_providers,
                    &mut pending_get_object,
                    &object_provider,
                    &pubsub,
                ).await;
            }
            command = commands.recv() => {
                let Some(command) = command else { break };
                handle_command(&mut swarm, command, &mut pending_get_providers, &mut pending_get_object, &peers, &relay_reservations);
            }
        }
    }
}

fn handle_command(
    swarm: &mut libp2p::Swarm<CanopeeBehaviour>,
    command: Command,
    pending_get_providers: &mut HashMap<kad::QueryId, oneshot::Sender<Vec<PeerId>>>,
    pending_get_object: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<ExportBundle>>,
    >,
    peers: &HashMap<PeerId, Peer>,
    relay_reservations: &HashMap<PeerId, RelayReservation>,
) {
    match command {
        Command::Dial(addr) => {
            if let Err(e) = swarm.dial(addr.clone()) {
                tracing::warn!("Failed to dial {addr}: {e}");
            }
        }
        Command::ListenViaRelay(relay_addr) => {
            let circuit_addr = relay_addr.with(libp2p::multiaddr::Protocol::P2pCircuit);
            if let Err(e) = swarm.listen_on(circuit_addr.clone()) {
                tracing::warn!("Failed to listen via relay {circuit_addr}: {e}");
            }
        }
        Command::FindProviders { object_id, reply } => {
            let key = kad::RecordKey::new(&object_id.0);
            let query_id = swarm.behaviour_mut().kad.get_providers(key);
            pending_get_providers.insert(query_id, reply);
        }
        Command::Announce(object_id) => {
            let key = kad::RecordKey::new(&object_id.0);
            if let Err(e) = swarm.behaviour_mut().kad.start_providing(key) {
                tracing::warn!("Failed to announce object {object_id}: {e}");
            }
        }
        Command::GetObject {
            peer_id,
            object_id,
            reply,
        } => {
            let request_id = swarm
                .behaviour_mut()
                .object_exchange
                .send_request(&peer_id, ObjectRequest::GetObject(object_id));
            pending_get_object.insert(request_id, reply);
        }
        Command::ListPeers(reply) => {
            let _ = reply.send(peers.values().cloned().collect());
        }
        Command::ListRelayReservations(reply) => {
            let _ = reply.send(relay_reservations.values().cloned().collect());
        }
        Command::Subscribe(topic, reply) => {
            let ident_topic = gossipsub::IdentTopic::new(topic);
            let result = swarm
                .behaviour_mut()
                .gossipsub
                .subscribe(&ident_topic)
                .map(|_| ())
                .map_err(|e| anyhow::anyhow!("Failed to subscribe: {e}"));
            let _ = reply.send(result);
        }
        Command::Unsubscribe(topic) => {
            let ident_topic = gossipsub::IdentTopic::new(topic);
            let _ = swarm.behaviour_mut().gossipsub.unsubscribe(&ident_topic);
        }
        Command::Publish { topic, data, reply } => {
            let ident_topic = gossipsub::IdentTopic::new(topic);
            let result = swarm
                .behaviour_mut()
                .gossipsub
                .publish(ident_topic, data)
                .map(|_| ())
                .map_err(|e| anyhow::anyhow!("Failed to publish: {e}"));
            let _ = reply.send(result);
        }
    }
}

async fn handle_swarm_event(
    event: SwarmEvent<CanopeeBehaviourEvent>,
    swarm: &mut libp2p::Swarm<CanopeeBehaviour>,
    peers: &mut HashMap<PeerId, Peer>,
    relay_reservations: &mut HashMap<PeerId, RelayReservation>,
    pending_get_providers: &mut HashMap<kad::QueryId, oneshot::Sender<Vec<PeerId>>>,
    pending_get_object: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<ExportBundle>>,
    >,
    object_provider: &Arc<dyn ObjectProvider>,
    pubsub: &broadcast::Sender<PubSubMessage>,
) {
    match event {
        SwarmEvent::ConnectionEstablished {
            peer_id, endpoint, ..
        } => {
            peers.entry(peer_id).or_insert_with(|| Peer::new(peer_id));
            swarm
                .behaviour_mut()
                .kad
                .add_address(&peer_id, endpoint.get_remote_address().clone());
        }
        SwarmEvent::ConnectionClosed { peer_id, .. } => {
            peers.remove(&peer_id);
        }
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
            for (peer_id, addr) in list {
                swarm.behaviour_mut().kad.add_address(&peer_id, addr.clone());
                if let Err(e) = swarm.dial(addr) {
                    tracing::debug!("Failed to dial mDNS peer {peer_id}: {e}");
                }
            }
        }
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Identify(identify::Event::Received {
            peer_id,
            info,
            ..
        })) => {
            for addr in &info.listen_addrs {
                swarm.behaviour_mut().kad.add_address(&peer_id, addr.clone());
            }
            let peer = peers.entry(peer_id).or_insert_with(|| Peer::new(peer_id));
            peer.addresses = info.listen_addrs;
        }
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::RelayClient(
            relay::client::Event::ReservationReqAccepted {
                relay_peer_id,
                renewal,
                ..
            },
        )) => {
            let reservation = relay_reservations
                .entry(relay_peer_id)
                .or_insert_with(|| RelayReservation::new(relay_peer_id));
            reservation.renewal = renewal;
            tracing::info!("Relay reservation accepted via {relay_peer_id} (renewal: {renewal})");
        }
        SwarmEvent::NewListenAddr { address, .. } => {
            if let Some(relay_peer_id) = relay_peer_id_from_circuit_addr(&address) {
                let reservation = relay_reservations
                    .entry(relay_peer_id)
                    .or_insert_with(|| RelayReservation::new(relay_peer_id));
                if !reservation.listen_addrs.contains(&address) {
                    reservation.listen_addrs.push(address);
                }
            }
        }
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Relay(relay_event)) => match relay_event {
            relay::Event::ReservationReqAccepted {
                src_peer_id,
                renewed,
            } => {
                tracing::info!(
                    "Relay: peer {src_peer_id} is now listening through us (renewed: {renewed})"
                );
            }
            relay::Event::ReservationReqDenied { src_peer_id } => {
                tracing::warn!("Relay: denied listen reservation for peer {src_peer_id}");
            }
            relay::Event::ReservationTimedOut { src_peer_id } => {
                tracing::info!("Relay: listen reservation for peer {src_peer_id} timed out");
            }
            relay::Event::CircuitReqAccepted {
                src_peer_id,
                dst_peer_id,
            } => {
                tracing::info!(
                    "Relay: peer {src_peer_id} dialed peer {dst_peer_id} through us"
                );
            }
            relay::Event::CircuitReqDenied {
                src_peer_id,
                dst_peer_id,
            } => {
                tracing::warn!(
                    "Relay: denied circuit request from {src_peer_id} to {dst_peer_id}"
                );
            }
            relay::Event::CircuitClosed {
                src_peer_id,
                dst_peer_id,
                error,
            } => {
                if let Some(error) = error {
                    tracing::info!(
                        "Relay: circuit from {src_peer_id} to {dst_peer_id} closed with error: {error}"
                    );
                } else {
                    tracing::info!("Relay: circuit from {src_peer_id} to {dst_peer_id} closed");
                }
            }
            _ => {}
        },
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Autonat(
            autonat::Event::StatusChanged { old, new },
        )) => {
            tracing::info!("NAT status changed: {old:?} -> {new:?}");
        }
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Dcutr(event)) => {
            tracing::info!("Hole punch event: {event:?}");
        }
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Gossipsub(gossipsub::Event::Message {
            message,
            ..
        })) => {
            let _ = pubsub.send(PubSubMessage {
                topic: message.topic.into_string(),
                source: message.source,
                data: message.data,
            });
        }
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Kad(
            kad::Event::OutboundQueryProgressed {
                id,
                result: kad::QueryResult::GetProviders(Ok(result)),
                ..
            },
        )) => {
            if let Some(reply) = pending_get_providers.remove(&id) {
                let providers = match result {
                    kad::GetProvidersOk::FoundProviders { providers, .. } => {
                        providers.into_iter().collect()
                    }
                    kad::GetProvidersOk::FinishedWithNoAdditionalRecord { .. } => Vec::new(),
                };
                let _ = reply.send(providers);
            }
        }
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::ObjectExchange(
            request_response::Event::Message { message, .. },
        )) => match message {
            request_response::Message::Response {
                request_id,
                response,
            } => {
                if let Some(reply) = pending_get_object.remove(&request_id) {
                    let result = match response {
                        ObjectResponse::Object(bundle) => Ok(bundle),
                        ObjectResponse::NotFound => Err(anyhow::anyhow!("Object not found")),
                    };
                    let _ = reply.send(result);
                }
            }
            request_response::Message::Request {
                request, channel, ..
            } => {
                let ObjectRequest::GetObject(object_id) = request;
                let response = match object_provider.get_object(&object_id).await {
                    Some(bundle) => ObjectResponse::Object(bundle),
                    None => ObjectResponse::NotFound,
                };
                let _ = swarm
                    .behaviour_mut()
                    .object_exchange
                    .send_response(channel, response);
            }
        },
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::ObjectExchange(
            request_response::Event::OutboundFailure {
                request_id, error, ..
            },
        )) => {
            if let Some(reply) = pending_get_object.remove(&request_id) {
                let _ = reply.send(Err(anyhow::anyhow!("Request failed: {error}")));
            }
        }
        _ => {}
    }
}
