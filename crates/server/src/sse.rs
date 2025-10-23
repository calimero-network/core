/*!
# Server-Sent Events (SSE) Module

Provides real-time event streaming to clients using Server-Sent Events (SSE).

## Architecture

This module is organized following SOLID principles:

- `config` - Configuration and constants (Single Responsibility)
- `session` - Session state management (Single Responsibility)
- `storage` - Persistent storage operations (Single Responsibility)
- `state` - Global service state (Dependency Inversion)
- `handlers` - HTTP request handlers (Interface Segregation)
- `events` - Node event processing (Single Responsibility)

## Features

- **Automatic Reconnection**: Clients can reconnect with Last-Event-ID header
- **Persistent Sessions**: Sessions survive server restarts
- **Session Expiry**: Automatic cleanup after 24 hours of inactivity
- **Multi-Context Subscriptions**: Subscribe to multiple contexts per session
- **Event IDs**: All events have unique IDs for tracking

## Usage

The SSE service is automatically initialized when enabled in server configuration.

```rust
use calimero_server::sse::SseConfig;

let config = SseConfig::new(true);
```
*/

pub mod config;
mod events;
mod handlers;
mod session;
mod state;
mod storage;

use axum::routing::{get, post};
use axum::Extension;
use axum::Router;
use calimero_node_primitives::client::NodeClient;
use calimero_store::Store;
use std::sync::Arc;
use tracing::info;

use crate::config::ServerConfig;

// Re-export public types
pub use config::SseConfig;
use handlers::{get_session_handler, handle_subscription, sse_handler};
use state::ServiceState;

/// Initialize SSE service
///
/// Creates the SSE router if enabled in configuration.
///
/// # Returns
///
/// `Some((path, router))` if SSE is enabled, `None` otherwise
#[must_use]
pub fn service(
    config: &ServerConfig,
    node_client: NodeClient,
    store: Store,
) -> Option<(&'static str, Router)> {
    let _ = match &config.sse {
        Some(config) if config.enabled => config,
        _ => {
            info!("SSE server is disabled");
            return None;
        }
    };

    let path = "/sse";

    for listen in &config.listen {
        info!("SSE server listening on {}/http{{{}}}", listen, path);
    }

    let state = Arc::new(ServiceState::new(node_client, store));

    let router = Router::new()
        .route("/", get(sse_handler))
        .route("/session/:session_id", get(get_session_handler))
        .route("/subscription", post(handle_subscription))
        .layer(Extension(state));

    Some((path, router))
}
