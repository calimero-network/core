use std::sync::Arc;

use calimero_network::client::NetworkClient;
use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::Result as EyreResult;
use libp2p::gossipsub::TopicHash;
use owo_colors::OwoColorize;

/// List the peers in the network
#[derive(Copy, Clone, Debug, Parser)]
pub struct PeersCommand {
    /// The context ID to list the peers for
    context_id: Option<ContextId>,
}

impl PeersCommand {
    pub async fn run(self, network_client: Arc<NetworkClient>) -> EyreResult<()> {
        let ind = ">>".blue();
        println!(
            "{ind} Peers (General): {:#?}",
            network_client.peer_count().await.cyan()
        );

        if let Some(context_id) = self.context_id {
            let topic = TopicHash::from_raw(context_id);
            println!(
                "{ind} Peers (Session) for Topic {}: {:#?}",
                topic.clone(),
                network_client.mesh_peer_count(topic).await.cyan()
            );
        }

        Ok(())
    }
}
