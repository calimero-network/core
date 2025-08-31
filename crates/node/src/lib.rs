#![allow(clippy::print_stdout, reason = "Acceptable for CLI")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "TODO: Check if this is necessary"
)]

use std::collections::BTreeMap;
use std::pin::pin;
use std::sync::Arc;

use actix::{Actor, AsyncContext, WrapFuture};
use calimero_blobstore::BlobManager;
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::blobs::BlobId;
use futures_util::StreamExt;
use tokio::sync::Mutex;
use tracing::error;

mod handlers;
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

    // -- blobs --
    // todo! potentially make this a dashmap::DashMap
    // todo! use cached::TimedSizedCache with a gc task
    blob_cache: BTreeMap<BlobId, Arc<Mutex<Option<Arc<[u8]>>>>>,
    // fixme! this should be opaque, so we can permit mmapping blobs
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

            blob_cache: BTreeMap::new(),
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
