use calimero_network::client::NetworkClient;
use clap::Parser;
use eyre::Result;
use libp2p::gossipsub::TopicHash;
use owo_colors::OwoColorize;
use std::sync::Arc;

#[derive(Debug, Parser)]
pub struct PeersCommand {
    topic: String,
}

impl PeersCommand {
    pub async fn run(self, network_client: Arc<NetworkClient>) -> Result<()> {
        let ind = ">>".blue();
        println!(
            "{ind} Peers (General): {:#?}",
            network_client.peer_count().await.cyan()
        );

        let topic = TopicHash::from_raw(self.topic.clone());
        println!(
            "{ind} Peers (Session) for Topic {}: {:#?}",
            topic.clone(),
            network_client.mesh_peer_count(topic).await.cyan()
        );

        Ok(())
    }
}
