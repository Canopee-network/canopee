use crate::node_client::NodeClient;
use canopee_protocol::{NodeCommand, NodeResponse, PubSubMessage};
use tokio::net::UnixStream;

/// A live gossipsub subscription. Dropping this closes the underlying
/// connection, which the node interprets as an unsubscribe.
pub struct Subscription {
    stream: UnixStream,
    topic: String,
}

impl Subscription {
    pub(crate) async fn open(client: &NodeClient, topic: String) -> anyhow::Result<Self> {
        let mut stream = client.connect().await?;
        let bytes = bincode::serialize(&NodeCommand::Subscribe {
            topic: topic.clone(),
        })?;
        NodeClient::write_frame(&mut stream, &bytes).await?;

        match NodeClient::read_frame(&mut stream).await? {
            NodeResponse::Subscribed => Ok(Self { stream, topic }),
            NodeResponse::Error { message } => Err(anyhow::anyhow!(message)),
            other => Err(anyhow::anyhow!("Unexpected response: {other:?}")),
        }
    }

    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Waits for the next message on this subscription's topic.
    /// Returns `None` only when the node closed the connection cleanly; a
    /// transport or decode failure is returned as an error so callers can
    /// tell "the node went away" from "the link broke".
    pub async fn next(&mut self) -> anyhow::Result<Option<PubSubMessage>> {
        match NodeClient::read_frame_opt(&mut self.stream).await? {
            Some(NodeResponse::PubSub(message)) => Ok(Some(message)),
            Some(other) => Err(anyhow::anyhow!("Unexpected response: {other:?}")),
            None => Ok(None),
        }
    }
}
