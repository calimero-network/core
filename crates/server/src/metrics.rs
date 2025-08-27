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
