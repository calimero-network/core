use calimero_node_primitives::client::NodeClient;
use calimero_server_primitives::sse::ConnectionId;
use calimero_store::Store;
use std::collections::HashMap;
use tokio::sync::RwLock;

use super::session::{ActiveConnection, SessionState};

/// Global SSE service state
pub struct ServiceState {
    pub node_client: NodeClient,
    pub store: Store,
    /// Session state persists across reconnections (in-memory cache)
    pub sessions: RwLock<HashMap<ConnectionId, SessionState>>,
    /// Active connections track current SSE streams
    pub active_connections: RwLock<HashMap<ConnectionId, ActiveConnection>>,
}

impl ServiceState {
    /// Create new service state
    #[must_use]
    pub fn new(node_client: NodeClient, store: Store) -> Self {
        Self {
            node_client,
            store,
            sessions: RwLock::default(),
            active_connections: RwLock::default(),
        }
    }
}
