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
pub mod sync;
pub mod types;

use sync::SyncConfig;

#[derive(Debug)]
pub struct NodeManager {
    pub sync_config: SyncConfig,

    pub datastore: Store,
    pub blobstore: BlobManager,

    pub context_client: ContextClient,
    pub node_client: NodeClient,

    // -- blobs --
    // todo! potentially make this a dashmap::DashMap
    // todo! use cached::TimedSizedCache with a gc task
    blob_cache: BTreeMap<BlobId, Arc<[u8]>>,
}

impl Actor for NodeManager {
    type Context = actix::Context<Self>;
}
