use std::collections::HashSet;
use std::sync::Arc;

use calimero_network_primitives::client::NetworkClient;
use libp2p::gossipsub::IdentTopic;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Tracks gossipsub topic subscriptions with deduplication.
#[derive(Clone, Debug)]
pub struct TopicManager {
    network_client: NetworkClient,
    subscribed: Arc<RwLock<HashSet<String>>>,
}

impl TopicManager {
    pub fn new(network_client: NetworkClient) -> Self {
        Self {
            network_client,
            subscribed: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Subscribe to a topic if not already subscribed.
    pub async fn ensure_subscribed(&self, topic: &str) -> eyre::Result<()> {
        {
            let subs = self.subscribed.read().await;
            if subs.contains(topic) {
                debug!(topic, "already subscribed, skipping");
                return Ok(());
            }
        }
        let ident_topic = IdentTopic::new(topic);
        let _ignored = self.network_client.subscribe(ident_topic).await?;
        self.subscribed.write().await.insert(topic.to_owned());
        info!(topic, "subscribed to topic");
        Ok(())
    }

    /// Unsubscribe from a topic.
    pub async fn unsubscribe(&self, topic: &str) -> eyre::Result<()> {
        let ident_topic = IdentTopic::new(topic);
        let _ignored = self.network_client.unsubscribe(ident_topic).await?;
        self.subscribed.write().await.remove(topic);
        info!(topic, "unsubscribed from topic");
        Ok(())
    }

    /// Check if subscribed to a topic.
    pub async fn is_subscribed(&self, topic: &str) -> bool {
        self.subscribed.read().await.contains(topic)
    }

    /// List all subscribed topics.
    pub async fn subscribed_topics(&self) -> Vec<String> {
        self.subscribed.read().await.iter().cloned().collect()
    }
}
