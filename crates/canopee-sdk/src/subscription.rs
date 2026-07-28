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
    /// Returns `None` if the node closed the connection.
    pub async fn next(&mut self) -> anyhow::Result<Option<PubSubMessage>> {
        loop {
            match NodeClient::read_frame(&mut self.stream).await {
                Ok(NodeResponse::PubSub(message)) => return Ok(Some(message)),
                Ok(other) => {
                    return Err(anyhow::anyhow!("Unexpected response: {other:?}"));
                }
                Err(_) => return Ok(None),
            }
        }
    }
}
