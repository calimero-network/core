#![allow(clippy::print_stdout, reason = "Acceptable for CLI")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "TODO: Check if this is necessary"
)]

use actix::Actor;
use calimero_blobstore::BlobManager;
use calimero_context::ContextManager;
use calimero_network::client::NetworkClient;
use calimero_network::NetworkManager;
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

    pub context_manager: ContextClient,
    pub network_manager: NetworkClient,
}

impl Actor for NodeManager {
    type Context = actix::Context<Self>;
}
