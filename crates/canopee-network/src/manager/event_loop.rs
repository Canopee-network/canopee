use super::{
    relay_peer_id_from_circuit_addr, Command, InboundPairing, InboundServe,
    InboundServeRegistration, ObjectProvider, ObjectStore,
};
use crate::behaviour::{CanopeeBehaviour, CanopeeBehaviourEvent};
use crate::message::{
    CanopeePairingResponse, ObjectRequest, ObjectResponse, PubSubMessage,
    ServeRegistrationResponse, ServeResponse,
};
use crate::peer::{Peer, RelayReservation};
use canopee_storage::ExportBundle;
use futures::{future::BoxFuture, stream::FuturesUnordered, StreamExt};
use libp2p::kad;
use libp2p::request_response;
use libp2p::swarm::SwarmEvent;
use libp2p::{autonat, gossipsub, identify, mdns, relay, Multiaddr, PeerId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};

type PairingReplyFuture = BoxFuture<
    'static,
    (
        request_response::ResponseChannel<CanopeePairingResponse>,
        CanopeePairingResponse,
    ),
>;
type ServeReplyFuture = BoxFuture<
    'static,
    (
        request_response::ResponseChannel<ServeResponse>,
        ServeResponse,
    ),
>;
type ServeRegistryReplyFuture = BoxFuture<
    'static,
    (
        request_response::ResponseChannel<ServeRegistrationResponse>,
        ServeRegistrationResponse,
    ),
>;

pub(super) async fn run_event_loop(
    mut swarm: libp2p::Swarm<CanopeeBehaviour>,
    mut commands: mpsc::Receiver<Command>,
    object_provider: Arc<dyn ObjectProvider>,
    object_store: Arc<dyn ObjectStore>,
    pubsub: broadcast::Sender<PubSubMessage>,
    pairing_events: broadcast::Sender<InboundPairing>,
    serve_events: broadcast::Sender<InboundServe>,
    serve_registry_events: broadcast::Sender<InboundServeRegistration>,
) {
    let mut peers: HashMap<PeerId, Peer> = HashMap::new();
    let mut listen_addrs: Vec<Multiaddr> = Vec::new();
    let mut relay_reservations: HashMap<PeerId, RelayReservation> = HashMap::new();
    let mut pending_get_providers: HashMap<kad::QueryId, oneshot::Sender<Vec<PeerId>>> =
        HashMap::new();
    let mut pending_put_record: HashMap<kad::QueryId, oneshot::Sender<anyhow::Result<()>>> =
        HashMap::new();
    let mut pending_get_record: HashMap<
        kad::QueryId,
        oneshot::Sender<anyhow::Result<Option<Vec<u8>>>>,
    > = HashMap::new();
    let mut pending_get_object: HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<ExportBundle>>,
    > = HashMap::new();
    let mut pending_replicate: HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<()>>,
    > = HashMap::new();
    let mut pending_pairing_request: HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<String>>,
    > = HashMap::new();
    let mut pending_serve_registration: HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<ServeRegistrationResponse>>,
    > = HashMap::new();
    let mut pending_serve_request: HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<ServeResponse>>,
    > = HashMap::new();
    let mut pairing_replies: FuturesUnordered<PairingReplyFuture> = FuturesUnordered::new();
    let mut serve_replies: FuturesUnordered<ServeReplyFuture> = FuturesUnordered::new();
    let mut serve_registry_replies: FuturesUnordered<ServeRegistryReplyFuture> =
        FuturesUnordered::new();
    let mut kad_bootstrapped = false;

    loop {
        tokio::select! {
            event = swarm.select_next_some() => {
                handle_swarm_event(
                    event,
                    &mut swarm,
                    &mut peers,
                    &mut listen_addrs,
                    &mut relay_reservations,
                    &mut pending_get_providers,
                    &mut pending_put_record,
                    &mut pending_get_record,
                    &mut pending_get_object,
                    &mut pending_replicate,
                    &mut pending_pairing_request,
                    &object_provider,
                    &object_store,
                    &pubsub,
                    &pairing_events,
                    &serve_events,
                    &serve_registry_events,
                    &mut pairing_replies,
                    &mut serve_replies,
                    &mut serve_registry_replies,
                    &mut pending_serve_registration,
                    &mut pending_serve_request,
                    &mut kad_bootstrapped,
                ).await;
            }
            command = commands.recv() => {
                let Some(command) = command else { break };
                handle_command(
                    &mut swarm,
                    command,
                    &mut pending_get_providers,
                    &mut pending_put_record,
                    &mut pending_get_record,
                    &mut pending_get_object,
                    &mut pending_replicate,
                    &mut pending_pairing_request,
                    &mut pending_serve_registration,
                    &mut pending_serve_request,
                    &mut peers,
                    &listen_addrs,
                    &relay_reservations,
                );
            }
            pairing_reply = async {
                if pairing_replies.is_empty() {
                    std::future::pending().await
                } else {
                    pairing_replies.next().await
                }
            } => {
                if let Some((channel, response)) = pairing_reply {
                    let _ = swarm
                        .behaviour_mut()
                        .pairing
                        .send_response(channel, response);
                }
            }
            serve_reply = async {
                if serve_replies.is_empty() {
                    std::future::pending().await
                } else {
                    serve_replies.next().await
                }
            } => {
                if let Some((channel, response)) = serve_reply {
                    let _ = swarm
                        .behaviour_mut()
                        .serve
                        .send_response(channel, response);
                }
            }
            serve_registry_reply = async {
                if serve_registry_replies.is_empty() {
                    std::future::pending().await
                } else {
                    serve_registry_replies.next().await
                }
            } => {
                if let Some((channel, response)) = serve_registry_reply {
                    let _ = swarm
                        .behaviour_mut()
                        .serve_registry
                        .send_response(channel, response);
                }
            }
        }
    }
}

pub(super) fn handle_command(
    swarm: &mut libp2p::Swarm<CanopeeBehaviour>,
    command: Command,
    pending_get_providers: &mut HashMap<kad::QueryId, oneshot::Sender<Vec<PeerId>>>,
    pending_put_record: &mut HashMap<kad::QueryId, oneshot::Sender<anyhow::Result<()>>>,
    pending_get_record: &mut HashMap<
        kad::QueryId,
        oneshot::Sender<anyhow::Result<Option<Vec<u8>>>>,
    >,
    pending_get_object: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<ExportBundle>>,
    >,
    pending_replicate: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<()>>,
    >,
    pending_pairing_request: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<String>>,
    >,
    pending_serve_registration: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<ServeRegistrationResponse>>,
    >,
    pending_serve_request: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<ServeResponse>>,
    >,
    peers: &mut HashMap<PeerId, Peer>,
    listen_addrs: &Vec<Multiaddr>,
    relay_reservations: &HashMap<PeerId, RelayReservation>,
) {
    match command {
        Command::Dial(addr) => {
            if let Err(e) = swarm.dial(addr.clone()) {
                tracing::warn!("Failed to dial {addr}: {e}");
            }
        }
        Command::ListListenAddresses(reply) => {
            let _ = reply.send(listen_addrs.clone());
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
        Command::Unannounce(object_id) => {
            let key = kad::RecordKey::new(&object_id.0);
            swarm.behaviour_mut().kad.stop_providing(&key);
        }
        Command::PutRecord { key, value, reply } => {
            let record = kad::Record::new(kad::RecordKey::new(&key), value);
            match swarm
                .behaviour_mut()
                .kad
                .put_record(record, kad::Quorum::One)
            {
                Ok(query_id) => {
                    pending_put_record.insert(query_id, reply);
                }
                Err(e) => {
                    let _ = reply.send(Err(anyhow::anyhow!("Failed to put record: {e}")));
                }
            }
        }
        Command::GetRecord { key, reply } => {
            let query_id = swarm
                .behaviour_mut()
                .kad
                .get_record(kad::RecordKey::new(&key));
            pending_get_record.insert(query_id, reply);
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
        Command::ReplicateObject {
            peer_id,
            bundle,
            reply,
        } => {
            // Targets: the explicitly named peer, or every currently connected
            // peer (the same set `peers()` reports — mDNS-discovered, dialed,
            // bootstrap relays, everything).
            let targets: Vec<PeerId> = match peer_id {
                Some(pid) => {
                    if peers.contains_key(&pid) {
                        vec![pid]
                    } else {
                        let _ = reply.send(Err(anyhow::anyhow!("peer {pid} is not connected")));
                        return;
                    }
                }
                None => peers.keys().copied().collect(),
            };
            if targets.is_empty() {
                // Nothing reachable to push to; the DHT announce path still
                // makes the object discoverable, so this is not an error.
                tracing::debug!("replicate_object: no connected peers to push to");
                let _ = reply.send(Ok(()));
                return;
            }
            let mut senders: Vec<(PeerId, oneshot::Receiver<anyhow::Result<()>>)> = Vec::new();
            for target in targets {
                let request_id = swarm
                    .behaviour_mut()
                    .object_exchange
                    .send_request(&target, ObjectRequest::Store(bundle.clone()));
                let (tx, rx) = oneshot::channel();
                pending_replicate.insert(request_id, tx);
                senders.push((target, rx));
            }
            // Collect the per-target verdicts off the event-loop thread: each
            // request_response round-trip is timeout-bounded by libp2p, so
            // this can never hang the loop. Failures surface as
            // `OutboundFailure` events, which resolve the `pending_replicate`
            // sender with an error.
            tokio::spawn(async move {
                let mut successes = 0usize;
                let mut first_error: Option<anyhow::Error> = None;
                for (target, rx) in senders {
                    match rx.await {
                        Ok(Ok(())) => successes += 1,
                        Ok(Err(e)) => {
                            tracing::warn!("peer {target} refused pushed object: {e}");
                            if first_error.is_none() {
                                first_error = Some(e);
                            }
                        }
                        Err(_) => {
                            tracing::warn!("peer {target} dropped the Store response");
                            if first_error.is_none() {
                                first_error =
                                    Some(anyhow::anyhow!("peer {target} dropped the response"));
                            }
                        }
                    }
                }
                // Broadcast is best-effort: at least one peer storing counts
                // as success (a connected relay/edge legitimately refuses).
                // An explicit single target is strict.
                let _ = reply.send(match first_error {
                    Some(e) if successes == 0 => Err(e),
                    _ => Ok(()),
                });
            });
        }
        Command::SendPairing {
            peer_id,
            request,
            reply,
        } => {
            let request_id = swarm
                .behaviour_mut()
                .pairing
                .send_request(&peer_id, request);
            pending_pairing_request.insert(request_id, reply);
        }
        Command::SendServeRegistration {
            edge_peer_id,
            registration,
            reply,
        } => {
            let request_id = swarm
                .behaviour_mut()
                .serve_registry
                .send_request(&edge_peer_id, registration);
            pending_serve_registration.insert(request_id, reply);
        }
        Command::SendServeRequest {
            peer_id,
            request,
            reply,
        } => {
            let request_id = swarm.behaviour_mut().serve.send_request(&peer_id, request);
            pending_serve_request.insert(request_id, reply);
        }
        Command::ListPeers(reply) => {
            let _ = reply.send(peers.values().cloned().collect());
        }
        Command::SetPeerMeta {
            peer_id,
            identity,
            username,
            display_name,
        } => {
            if let Some(peer) = peers.get_mut(&peer_id) {
                if let Some(identity) = identity {
                    peer.identity = Some(identity);
                }
                if let Some(username) = username {
                    peer.username = Some(username);
                }
                if let Some(display_name) = display_name {
                    peer.display_name = Some(display_name);
                }
            }
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

pub(super) async fn handle_swarm_event(
    event: SwarmEvent<CanopeeBehaviourEvent>,
    swarm: &mut libp2p::Swarm<CanopeeBehaviour>,
    peers: &mut HashMap<PeerId, Peer>,
    listen_addrs: &mut Vec<Multiaddr>,
    relay_reservations: &mut HashMap<PeerId, RelayReservation>,
    pending_get_providers: &mut HashMap<kad::QueryId, oneshot::Sender<Vec<PeerId>>>,
    pending_put_record: &mut HashMap<kad::QueryId, oneshot::Sender<anyhow::Result<()>>>,
    pending_get_record: &mut HashMap<
        kad::QueryId,
        oneshot::Sender<anyhow::Result<Option<Vec<u8>>>>,
    >,
    pending_get_object: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<ExportBundle>>,
    >,
    pending_replicate: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<()>>,
    >,
    pending_pairing_request: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<String>>,
    >,
    object_provider: &Arc<dyn ObjectProvider>,
    object_store: &Arc<dyn ObjectStore>,
    pubsub: &broadcast::Sender<PubSubMessage>,
    pairing_events: &broadcast::Sender<InboundPairing>,
    serve_events: &broadcast::Sender<InboundServe>,
    serve_registry_events: &broadcast::Sender<InboundServeRegistration>,
    pairing_replies: &mut FuturesUnordered<PairingReplyFuture>,
    serve_replies: &mut FuturesUnordered<ServeReplyFuture>,
    serve_registry_replies: &mut FuturesUnordered<ServeRegistryReplyFuture>,
    pending_serve_registration: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<ServeRegistrationResponse>>,
    >,
    pending_serve_request: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<anyhow::Result<ServeResponse>>,
    >,
    kad_bootstrapped: &mut bool,
) {
    match event {
        SwarmEvent::NewListenAddr { address, .. } => {
            if !listen_addrs.contains(&address) {
                listen_addrs.push(address.clone());
            }
            if let Some(relay_peer_id) = relay_peer_id_from_circuit_addr(&address) {
                let reservation = relay_reservations
                    .entry(relay_peer_id)
                    .or_insert_with(|| RelayReservation::new(relay_peer_id));
                if !reservation.listen_addrs.contains(&address) {
                    reservation.listen_addrs.push(address);
                }
            }
        }
        SwarmEvent::IncomingConnectionError {
            connection_id: _,
            local_addr,
            send_back_addr: _,
            error,
        } => {
            tracing::debug!("Incoming connection error on {local_addr}: {error}");
        }
        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
            tracing::debug!("Outgoing connection error to {peer_id:?}: {error}");
        }
        SwarmEvent::ConnectionEstablished {
            peer_id, endpoint, ..
        } => {
            if peer_id == *swarm.local_peer_id() {
                return;
            }
            peers.entry(peer_id).or_insert_with(|| Peer::new(peer_id));
            swarm
                .behaviour_mut()
                .kad
                .add_address(&peer_id, endpoint.get_remote_address().clone());

            // Once we've connected to a peer (in practice the bootstrap
            // relay), kick off a Kademlia bootstrap query to populate the
            // routing table. This turns the single dialed connection into
            // full DHT discovery so the node can find arbitrary peers.
            if !*kad_bootstrapped && endpoint.is_dialer() {
                *kad_bootstrapped = true;
                match swarm.behaviour_mut().kad.bootstrap() {
                    Ok(_) => tracing::info!("Kademlia bootstrap started"),
                    Err(e) => {
                        // No known peers — shouldn't happen since we just
                        // added one, but don't treat it as fatal.
                        tracing::debug!("Kademlia bootstrap could not start: {e}");
                    }
                }
            }
        }
        SwarmEvent::ConnectionClosed { peer_id, .. } => {
            peers.remove(&peer_id);
        }
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
            for (peer_id, addr) in list {
                swarm
                    .behaviour_mut()
                    .kad
                    .add_address(&peer_id, addr.clone());
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
            if peer_id == *swarm.local_peer_id() {
                return;
            }
            for addr in &info.listen_addrs {
                swarm
                    .behaviour_mut()
                    .kad
                    .add_address(&peer_id, addr.clone());
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
                tracing::info!("Relay: peer {src_peer_id} dialed peer {dst_peer_id} through us");
            }
            relay::Event::CircuitReqDenied {
                src_peer_id,
                dst_peer_id,
            } => {
                tracing::warn!("Relay: denied circuit request from {src_peer_id} to {dst_peer_id}");
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
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Autonat(autonat::Event::StatusChanged {
            old,
            new,
        })) => {
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
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Kad(
            kad::Event::OutboundQueryProgressed {
                id,
                result: kad::QueryResult::PutRecord(result),
                ..
            },
        )) => {
            if let Some(reply) = pending_put_record.remove(&id) {
                let result = result
                    .map(|_| ())
                    .map_err(|e| anyhow::anyhow!("Failed to put record: {e}"));
                let _ = reply.send(result);
            }
        }
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Kad(
            kad::Event::OutboundQueryProgressed {
                id,
                result: kad::QueryResult::GetRecord(result),
                ..
            },
        )) => {
            if let Some(reply) = pending_get_record.remove(&id) {
                let result = match result {
                    Ok(kad::GetRecordOk::FoundRecord(peer_record)) => {
                        Ok(Some(peer_record.record.value))
                    }
                    Ok(kad::GetRecordOk::FinishedWithNoAdditionalRecord { .. }) => Ok(None),
                    Err(kad::GetRecordError::NotFound { .. }) => Ok(None),
                    Err(e) => Err(anyhow::anyhow!("Failed to get record: {e}")),
                };
                let _ = reply.send(result);
            }
        }
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::ObjectExchange(
            request_response::Event::Message { message, .. },
        )) => match message {
            request_response::Message::Response {
                request_id,
                response,
            } => {
                if let Some(reply) = pending_replicate.remove(&request_id) {
                    let result = match response {
                        ObjectResponse::Stored => Ok(()),
                        ObjectResponse::StoreFailed(e) => {
                            Err(anyhow::anyhow!("Pushed object refused by peer: {e}"))
                        }
                        ObjectResponse::Object(_) | ObjectResponse::NotFound => {
                            Err(anyhow::anyhow!(
                                "peer answered a Store request with a GetObject response"
                            ))
                        }
                    };
                    let _ = reply.send(result);
                } else if let Some(reply) = pending_get_object.remove(&request_id) {
                    let result = match response {
                        ObjectResponse::Object(bundle) => Ok(bundle),
                        ObjectResponse::NotFound => Err(anyhow::anyhow!("Object not found")),
                        ObjectResponse::Stored | ObjectResponse::StoreFailed(_) => {
                            Err(anyhow::anyhow!(
                                "peer answered a GetObject request with a Store response"
                            ))
                        }
                    };
                    let _ = reply.send(result);
                }
            }
            request_response::Message::Request {
                request, channel, ..
            } => {
                match request {
                    ObjectRequest::GetObject(object_id) => {
                        let response = match object_provider.get_object(&object_id).await {
                            Some(bundle) => ObjectResponse::Object(bundle),
                            None => ObjectResponse::NotFound,
                        };
                        let _ = swarm
                            .behaviour_mut()
                            .object_exchange
                            .send_response(channel, response);
                    }
                    // A peer actively pushing an object to us. `put_verified`
                    // both verifies the signature and persists, so an invalid
                    // push is refused here rather than trusted on arrival.
                    ObjectRequest::Store(bundle) => {
                        let response = match object_store.put_verified(&bundle.object).await {
                            Ok(()) => ObjectResponse::Stored,
                            Err(e) => {
                                tracing::warn!(
                                    "Refusing pushed object {} from a peer: {e:#}",
                                    bundle.object.id
                                );
                                ObjectResponse::StoreFailed(e.to_string())
                            }
                        };
                        let _ = swarm
                            .behaviour_mut()
                            .object_exchange
                            .send_response(channel, response);
                    }
                }
            }
        },
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::ObjectExchange(
            request_response::Event::OutboundFailure {
                request_id, error, ..
            },
        )) => {
            if let Some(reply) = pending_replicate.remove(&request_id) {
                let _ = reply.send(Err(anyhow::anyhow!("Store request failed: {error}")));
            } else if let Some(reply) = pending_get_object.remove(&request_id) {
                let _ = reply.send(Err(anyhow::anyhow!("Request failed: {error}")));
            }
        }
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Pairing(
            request_response::Event::Message { peer, message, .. },
        )) => match message {
            request_response::Message::Request {
                request, channel, ..
            } => {
                // Forward to the runtime's pairing handler, which decrypts the
                // payload and decides whether to accept. The held response
                // channel + reply `rx` are wrapped into a future polled by the
                // select loop (busy-loop-safe via `is_empty()` guard) so once
                // the handler answers we route the response over the wire.
                let (reply_tx, mut reply_rx) = mpsc::unbounded_channel();
                if pairing_events
                    .send(InboundPairing {
                        peer,
                        request,
                        reply: reply_tx,
                    })
                    .is_err()
                {
                    tracing::warn!("No pairing handler subscribed; ignoring inbound pairing");
                }
                pairing_replies.push(Box::pin(async move {
                    let response = reply_rx
                        .recv()
                        .await
                        .unwrap_or(CanopeePairingResponse::Error(
                            "pairing handler unavailable".into(),
                        ));
                    (channel, response)
                }));
                // A bounded sender means the request already timed out on the
                // sender's side; the future above completes with an Error
                // response that the behaviour drops, so nothing leaks.
            }
            request_response::Message::Response {
                request_id,
                response,
            } => {
                if let Some(reply) = pending_pairing_request.remove(&request_id) {
                    let result = match response {
                        CanopeePairingResponse::Accepted(message) => Ok(message),
                        CanopeePairingResponse::Error(e) => {
                            Err(anyhow::anyhow!("Pairing refused by remote: {e}"))
                        }
                    };
                    let _ = reply.send(result);
                }
            }
        },
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Pairing(
            request_response::Event::OutboundFailure {
                request_id, error, ..
            },
        )) => {
            if let Some(reply) = pending_pairing_request.remove(&request_id) {
                let _ = reply.send(Err(anyhow::anyhow!("Pairing request failed: {error}")));
            }
        }
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Serve(request_response::Event::Message {
            peer,
            message,
            ..
        })) => match message {
            request_response::Message::Request {
                request, channel, ..
            } => {
                let (reply_tx, mut reply_rx) = mpsc::unbounded_channel();
                if serve_events
                    .send(InboundServe {
                        peer,
                        request,
                        reply: reply_tx,
                    })
                    .is_err()
                {
                    tracing::warn!("No serve session subscribed; ignoring inbound request");
                }
                serve_replies.push(Box::pin(async move {
                    let response = reply_rx.recv().await.unwrap_or_else(|| ServeResponse {
                        status: "500 Internal Server Error".into(),
                        headers: Vec::new(),
                        body: b"no serving node at this address".to_vec(),
                    });
                    (channel, response)
                }));
            }
            request_response::Message::Response {
                request_id,
                response,
            } => {
                if let Some(reply) = pending_serve_request.remove(&request_id) {
                    let _ = reply.send(Ok(response));
                }
            }
        },
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::Serve(
            request_response::Event::OutboundFailure {
                request_id, error, ..
            },
        )) => {
            if let Some(reply) = pending_serve_request.remove(&request_id) {
                let _ = reply.send(Err(anyhow::anyhow!("Serve request failed: {error}")));
            }
        }
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::ServeRegistry(
            request_response::Event::Message { peer, message, .. },
        )) => match message {
            request_response::Message::Request {
                request, channel, ..
            } => {
                let (reply_tx, mut reply_rx) = mpsc::unbounded_channel();
                if serve_registry_events
                    .send(InboundServeRegistration {
                        peer,
                        request,
                        reply: reply_tx,
                    })
                    .is_err()
                {
                    tracing::warn!("No registry handler subscribed; rejecting registration");
                }
                serve_registry_replies.push(Box::pin(async move {
                    let response = reply_rx.recv().await.unwrap_or_else(|| {
                        ServeRegistrationResponse::Error("this node is not a Canopee edge".into())
                    });
                    (channel, response)
                }));
            }
            request_response::Message::Response {
                request_id,
                response,
            } => {
                if let Some(reply) = pending_serve_registration.remove(&request_id) {
                    let _ = reply.send(Ok(response));
                }
            }
        },
        SwarmEvent::Behaviour(CanopeeBehaviourEvent::ServeRegistry(
            request_response::Event::OutboundFailure {
                request_id, error, ..
            },
        )) => {
            if let Some(reply) = pending_serve_registration.remove(&request_id) {
                let _ = reply.send(Err(anyhow::anyhow!(
                    "Serve registration request failed: {error}"
                )));
            }
        }
        _ => {}
    }
}
