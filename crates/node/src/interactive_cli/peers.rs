use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::Result as EyreResult;
use libp2p::gossipsub::TopicHash;
use owo_colors::OwoColorize;

use crate::Node;

/// List the peers in the network
#[derive(Copy, Clone, Debug, Parser)]
pub struct PeersCommand {
    /// The context to list the peers for
    context: Option<Alias<ContextId>>,
}

impl PeersCommand {
    pub async fn run(self, node: &Node) -> EyreResult<()> {
        let ind = ">>".blue();
        println!(
            "{ind} Peers (General): {:#?}",
            node.network_client.peer_count().await.cyan()
        );

        let context_id = self
            .context
            .map(|context| node.ctx_manager.resolve_alias(context, None))
            .transpose()?
            .flatten();

        if let Some(context_id) = context_id {
            let topic = TopicHash::from_raw(context_id);
            println!(
                "{ind} Peers (Session) for Topic {}: {:#?}",
                topic.clone(),
                node.network_client.mesh_peer_count(topic).await.cyan()
            );
        }

        Ok(())
    }
}
