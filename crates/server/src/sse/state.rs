use calimero_node_primitives::client::NodeClient;
use calimero_server_primitives::sse::ConnectionId;
use calimero_store::Store;
use std::collections::HashMap;
use tokio::sync::RwLock;

use super::session::SessionState;

/// Global SSE service state
pub struct ServiceState {
    pub node_client: NodeClient,
    pub store: Store,
    /// Session state persists across reconnections (in-memory cache)
    pub sessions: RwLock<HashMap<ConnectionId, SessionState>>,
}

impl ServiceState {
    /// Create new service state
    #[must_use]
    pub fn new(node_client: NodeClient, store: Store) -> Self {
        Self {
            node_client,
            store,
            sessions: RwLock::default(),
        }
    }
}
