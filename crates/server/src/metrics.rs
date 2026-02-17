//! Prometheus metrics service.
//!
//! Exposes the Prometheus registry at `/metrics` endpoint for scraping.
//!
//! # Sync Metrics Integration
//!
//! To add sync metrics to the registry, use `calimero_node::sync::PrometheusSyncMetrics`:
//!
//! ```rust,ignore
//! use calimero_node::sync::PrometheusSyncMetrics;
//! use prometheus_client::registry::Registry;
//!
//! let mut registry = Registry::default();
//! let sync_metrics = PrometheusSyncMetrics::new(&mut registry);
//!
//! // Pass `sync_metrics` to SyncManager during initialization
//! // Pass `registry` to the server for metrics exposition
//! ```
//!
//! Available sync metrics:
//! - `sync_messages_sent_total{protocol}`: Messages sent by protocol type
//! - `sync_bytes_sent_total{protocol}`: Bytes sent
//! - `sync_round_trips_total{protocol}`: Round trips
//! - `sync_entities_transferred_total`: Entities transferred
//! - `sync_merges_total{crdt_type}`: CRDT merges by type
//! - `sync_phase_duration_seconds{phase}`: Phase timing histogram
//! - `sync_snapshot_blocked_total`: I5 protection triggers
//! - `sync_verification_failures_total`: I7 violations
//! - `sync_buffer_drops_total`: I6 violation risk events

use std::sync::Arc;

use axum::response::IntoResponse;
use axum::routing::{get, Router};
use axum::Extension;
use prometheus_client::encoding::text::encode;
use prometheus_client::registry::Registry;
use tracing::info;

use crate::config::ServerConfig;

pub(crate) struct ServiceState {
    registry: Registry,
}

pub(crate) fn service(config: &ServerConfig, registry: Registry) -> Option<(&'static str, Router)> {
    let path = "/metrics"; // todo! source from config

    for listen in &config.listen {
        info!("Metrics server listening on {}/http{{{}}}", listen, path);
    }

    let state = Arc::new(ServiceState { registry });
    let handler = get(handle_request).layer(Extension(Arc::clone(&state)));

    let router = Router::new().route("/", handler);

    Some((path, router))
}

async fn handle_request(Extension(state): Extension<Arc<ServiceState>>) -> impl IntoResponse {
    let mut buffer = String::new();
    encode(&mut buffer, &state.registry).unwrap();
    buffer
}
