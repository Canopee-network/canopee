use crate::node_client::NodeClient;
use crate::subscription::Subscription;
use canopee_identity::IdentityId;
use canopee_protocol::{NodeCommand, NodeResponse, PeerInfo, RelayReservationInfo};
use canopee_storage::{ExportBundle, Object, ObjectId, ObjectInfo};

/// Entry point for apps that want to use a Canopee node's identity, storage,
/// and network capabilities. Talks to the locally running node over its Unix
/// socket; the node itself owns the identity keys, object storage, and the
/// libp2p swarm.
pub struct CanopeeClient {
    node: NodeClient,
}

impl CanopeeClient {
    /// Connects to the local Canopee node. Returns an error if the node
    /// isn't running (call `canopee start` / `canopee-node` first).
    pub async fn connect() -> anyhow::Result<Self> {
        let node = NodeClient::new().await?;
        if !node.is_running().await {
            anyhow::bail!("Canopee node is not running");
        }
        Ok(Self { node })
    }

    async fn request(&self, command: NodeCommand) -> anyhow::Result<NodeResponse> {
        self.node.request(command).await
    }

    fn unexpected(response: NodeResponse) -> anyhow::Error {
        match response {
            NodeResponse::Error { message } => anyhow::anyhow!(message),
            other => anyhow::anyhow!("Unexpected response: {other:?}"),
        }
    }

    // ---- identity ----

    /// The identity of the local node this client is connected to.
    pub async fn identity(&self) -> anyhow::Result<IdentityId> {
        match self.request(NodeCommand::Identity).await? {
            NodeResponse::Identity { identity_id } => Ok(identity_id),
            other => Err(Self::unexpected(other)),
        }
    }

    // ---- storage ----

    /// Stores data locally as a signed object, owned by the node's identity.
    pub async fn put(&self, data: Vec<u8>) -> anyhow::Result<ObjectId> {
        match self.request(NodeCommand::Put { data }).await? {
            NodeResponse::ObjectCreated { id } => Ok(id),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Reads a locally stored object.
    pub async fn get(&self, id: ObjectId) -> anyhow::Result<Object> {
        match self.request(NodeCommand::Get { id }).await? {
            NodeResponse::Object { object } => Ok(object),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Lists all objects held in local storage.
    pub async fn list(&self) -> anyhow::Result<Vec<ObjectInfo>> {
        match self.request(NodeCommand::List).await? {
            NodeResponse::Objects { objects } => Ok(objects),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Exports a locally stored object as a portable, self-verifying bundle.
    pub async fn export(&self, id: ObjectId) -> anyhow::Result<ExportBundle> {
        match self.request(NodeCommand::Export { id }).await? {
            NodeResponse::Exported { bundle } => Ok(bundle),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Imports an object bundle (e.g. one received from a peer) into local storage.
    pub async fn import(&self, bundle: ExportBundle) -> anyhow::Result<()> {
        match self.request(NodeCommand::Import { bundle }).await? {
            NodeResponse::Imported => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    // ---- network ----

    /// Peers currently connected to the node's libp2p swarm.
    pub async fn peers(&self) -> anyhow::Result<Vec<PeerInfo>> {
        match self.request(NodeCommand::Peers).await? {
            NodeResponse::Peers { peers } => Ok(peers),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Dials a peer directly at the given multiaddr
    /// (e.g. `/ip4/1.2.3.4/tcp/4001/p2p/<peer id>`).
    pub async fn dial(&self, addr: impl Into<String>) -> anyhow::Result<()> {
        match self
            .request(NodeCommand::Dial { addr: addr.into() })
            .await?
        {
            NodeResponse::Dialed => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Requests a circuit reservation through a relay so peers behind other
    /// NATs can reach this node, enabling hole punching (dcutr) to upgrade
    /// to a direct connection.
    pub async fn listen_via_relay(&self, relay_addr: impl Into<String>) -> anyhow::Result<()> {
        match self
            .request(NodeCommand::ListenViaRelay {
                relay_addr: relay_addr.into(),
            })
            .await?
        {
            NodeResponse::ListeningViaRelay => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Lists accepted relay circuit reservations. Use this to confirm a
    /// `listen_via_relay` request actually succeeded (and to get the
    /// resulting dialable `/p2p-circuit` addresses) before asking others to
    /// dial you through that relay.
    pub async fn relay_reservations(&self) -> anyhow::Result<Vec<RelayReservationInfo>> {
        match self.request(NodeCommand::RelayReservations).await? {
            NodeResponse::RelayReservations { reservations } => Ok(reservations),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Announces on the DHT that this node holds the given object, so other
    /// peers can discover it via [`Self::find_providers`].
    pub async fn announce(&self, id: ObjectId) -> anyhow::Result<()> {
        match self.request(NodeCommand::Announce { id }).await? {
            NodeResponse::Announced => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Finds peers that have announced the given object.
    pub async fn find_providers(&self, id: ObjectId) -> anyhow::Result<Vec<String>> {
        match self.request(NodeCommand::FindProviders { id }).await? {
            NodeResponse::Providers { peer_ids } => Ok(peer_ids),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Fetches an object directly from a specific peer (typically one found
    /// via [`Self::find_providers`]) and stores it locally.
    pub async fn fetch_object(
        &self,
        peer_id: impl Into<String>,
        id: ObjectId,
    ) -> anyhow::Result<ExportBundle> {
        match self
            .request(NodeCommand::FetchObject {
                peer_id: peer_id.into(),
                id,
            })
            .await?
        {
            NodeResponse::Exported { bundle } => Ok(bundle),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Publishes data to a gossipsub topic. Peers must be connected and
    /// subscribed to the same topic to receive it.
    pub async fn publish(&self, topic: impl Into<String>, data: Vec<u8>) -> anyhow::Result<()> {
        match self
            .request(NodeCommand::Publish {
                topic: topic.into(),
                data,
            })
            .await?
        {
            NodeResponse::Published => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Subscribes to a gossipsub topic. The returned [`Subscription`] streams
    /// messages until dropped or the node disconnects.
    pub async fn subscribe(&self, topic: impl Into<String>) -> anyhow::Result<Subscription> {
        Subscription::open(&self.node, topic.into()).await
    }

    /// Requests the local node to shut down.
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        match self.request(NodeCommand::Shutdown).await? {
            NodeResponse::ShutdownAccepted => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }
}
