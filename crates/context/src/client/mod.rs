//! Client-side functionality for Calimero contexts

pub mod config;
pub mod crypto;
pub mod external;
pub mod transport;

pub use config::ClientConfig;
pub use external::ExternalClient;
pub use transport::Transport;

use calimero_node_primitives::client::NodeClient;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;

use crate::messages::ContextMessage;
use transport::AnyTransport;

#[derive(Clone, Debug)]
pub struct ContextClient {
    datastore: Store,
    node_client: NodeClient,
    external_client: ExternalClient<AnyTransport>,
    context_manager: LazyRecipient<ContextMessage>,
}

impl ContextClient {
    pub fn new(
        datastore: Store,
        node_client: NodeClient,
        external_client: ExternalClient<AnyTransport>,
        context_manager: LazyRecipient<ContextMessage>,
    ) -> Self {
        Self {
            datastore,
            node_client,
            external_client,
            context_manager,
        }
    }

    // Temporary stub to satisfy server usage during merge
    pub fn get_context(&self, _id: &calimero_primitives::context::ContextId) -> eyre::Result<Option<calimero_primitives::context::Context>> {
        Ok(None)
    }
}