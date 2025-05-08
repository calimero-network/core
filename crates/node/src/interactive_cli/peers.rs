use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::Result as EyreResult;
use libp2p::gossipsub::TopicHash;
use owo_colors::OwoColorize;

/// List the peers in the network
#[derive(Copy, Clone, Debug, Parser)]
pub struct PeersCommand {
    /// The context to list the peers for
    context: Option<Alias<ContextId>>,
}

impl PeersCommand {
    pub async fn run(self, node_client: &NodeClient) -> EyreResult<()> {
        let ind = ">>".blue();
        
        let context_id = self
            .context
            .map(|context| node_client.resolve_alias(context, None))
            .transpose()?
            .flatten();

        println!(
            "{ind} Peers (General): {:#?}",
            node_client.get_peers_count(context_id).await.cyan()
        );


        if let Some(context_id) = context_id {
            let topic = TopicHash::from_raw(context_id);
            println!(
                "{ind} Peers (Session) for Topic {}: {:#?}",
                topic.clone(),
                node_client.get_peers_count(None).await.cyan()
            );
        }

        Ok(())
    }
}
