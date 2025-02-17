use calimero_primitives::alias::Kind;
use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::Result as EyreResult;
use libp2p::gossipsub::TopicHash;
use owo_colors::OwoColorize;

use crate::interactive_cli::commons::resolve_identifier;
use crate::Node;

/// List the peers in the network
#[derive(Clone, Debug, Parser)]
pub struct PeersCommand {
    /// The context ID to list the peers for
    context_id: Option<String>,
}

impl PeersCommand {
    pub async fn run(self, node: &Node) -> EyreResult<()> {
        let ind = ">>".blue();
        println!(
            "{ind} Peers (General): {:#?}",
            node.network_client.peer_count().await.cyan()
        );

        let context_id: Option<ContextId> = self
            .context_id
            .map(|context_inner| resolve_identifier(node, &context_inner, Kind::Context, None))
            .transpose()?
            .map(|hash| hash.into());

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
