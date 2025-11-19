//! Mock relayer implementation for local development
//!
//! This module provides an in-memory mock implementation of the relayer
//! that can be used for local development without requiring a live blockchain.

use std::sync::Arc;

use calimero_context_config::client::relayer::RelayRequest;
use eyre::Result as EyreResult;
use tokio::sync::RwLock;

mod handlers;
mod state;

#[cfg(test)]
mod tests;

use handlers::MockHandlers;
use state::MockState;

/// Mock relayer that handles requests in-memory
#[derive(Debug, Clone)]
pub struct MockRelayer {
    state: Arc<RwLock<MockState>>,
}

impl MockRelayer {
    /// Create a new mock relayer with empty state
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(MockState::new())),
        }
    }

    /// Handle a relay request and return the response
    pub async fn handle_request(&self, request: RelayRequest<'_>) -> EyreResult<Vec<u8>> {
        let mut state = self.state.write().await;

        MockHandlers::handle_operation(&mut state, &request.operation, &request.payload)
    }
}

impl Default for MockRelayer {
    fn default() -> Self {
        Self::new()
    }
}
