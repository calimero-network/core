use std::sync::Arc;

use calimero_network::client::NetworkClient;
use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::Result;
use libp2p::gossipsub::TopicHash;
use owo_colors::OwoColorize;

#[derive(Copy, Clone, Debug, Parser)]
pub struct PeersCommand {
    topic: Option<ContextId>,
}

impl PeersCommand {
    pub async fn run(self, network_client: Arc<NetworkClient>) -> Result<()> {
        let ind = ">>".blue();
        println!(
            "{ind} Peers (General): {:#?}",
            network_client.peer_count().await.cyan()
        );

        if let Some(topic) = self.topic {
            let topic = TopicHash::from_raw(topic);
            println!(
                "{ind} Peers (Session) for Topic {}: {:#?}",
                topic.clone(),
                network_client.mesh_peer_count(topic).await.cyan()
            );
        }

        Ok(())
    }
}
