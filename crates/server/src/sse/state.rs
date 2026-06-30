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
    /// Whether the auth guard is mounted in front of this service. When `true`,
    /// a request that resolves no authenticated principal is treated as an
    /// unauthenticated caller and fails session-ownership checks closed (rather
    /// than receiving the single-tenant "no principal" allowance).
    pub auth_enabled: bool,
}

impl ServiceState {
    /// Create new service state
    #[must_use]
    pub fn new(node_client: NodeClient, store: Store, auth_enabled: bool) -> Self {
        Self {
            node_client,
            store,
            sessions: RwLock::default(),
            auth_enabled,
        }
    }
}
