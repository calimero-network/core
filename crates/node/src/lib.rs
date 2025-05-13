#![allow(clippy::print_stdout, reason = "Acceptable for CLI")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "TODO: Check if this is necessary"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use actix::Actor;
use calimero_blobstore::BlobManager;
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::blobs::BlobId;
use calimero_store::Store;

pub mod handlers;
pub mod interactive_cli;
pub mod runtime_compat;
// pub mod sync;
pub mod types;
// fixme! here temporarily until interactive_cli moves to merod
mod temp;

// use sync::SyncConfig;
pub use temp::{start, NodeConfig};
use tokio::sync::Mutex;

#[derive(Debug)]
pub struct NodeManager {
    // sync_config: SyncConfig,
    //
    datastore: Store,
    blobstore: BlobManager,

    context_client: ContextClient,
    node_client: NodeClient,

    // -- blobs --
    // todo! potentially make this a dashmap::DashMap
    // todo! use cached::TimedSizedCache with a gc task
    blob_cache: BTreeMap<BlobId, Arc<Mutex<Option<Arc<[u8]>>>>>,
}

impl NodeManager {
    pub fn new(
        // sync_config: SyncConfig,
        datastore: Store,
        blobstore: BlobManager,
        context_client: ContextClient,
        node_client: NodeClient,
    ) -> Self {
        Self {
            // sync_config,
            datastore,
            blobstore,
            context_client,
            node_client,

            blob_cache: BTreeMap::new(),
        }
    }
}

impl Actor for NodeManager {
    type Context = actix::Context<Self>;
}
