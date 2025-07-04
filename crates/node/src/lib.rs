#![allow(clippy::print_stdout, reason = "Acceptable for CLI")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "TODO: Check if this is necessary"
)]

use std::pin::pin;

use actix::{Actor, AsyncContext, WrapFuture};
use calimero_blobstore::BlobManager;
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use futures_util::StreamExt;
use tracing::error;

pub mod handlers;
pub mod interactive_cli;
mod run;
pub mod sync;
mod utils;

pub use run::{start, NodeConfig};
use sync::SyncManager;

#[derive(Debug)]
pub struct NodeManager {
    blobstore: BlobManager,
    sync_manager: SyncManager,

    context_client: ContextClient,
    node_client: NodeClient,
}

impl NodeManager {
    pub(crate) fn new(
        blobstore: BlobManager,
        sync_manager: SyncManager,
        context_client: ContextClient,
        node_client: NodeClient,
    ) -> Self {
        Self {
            blobstore,
            sync_manager,
            context_client,
            node_client,
        }
    }
}

impl Actor for NodeManager {
    type Context = actix::Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let node_client = self.node_client.clone();

        let contexts = self.context_client.get_contexts(None);

        let _ignored = ctx.spawn(
            async move {
                let mut contexts = pin!(contexts);

                while let Some(context_id) = contexts.next().await {
                    let Ok(context_id) = context_id else { continue };

                    if let Err(err) = node_client.subscribe(&context_id).await {
                        error!("Failed to subscribe to context {}: {}", context_id, err);
                    }
                }
            }
            .into_actor(self),
        );
    }
}
