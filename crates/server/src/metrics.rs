//! Prometheus metrics service.
//!
//! Exposes the Prometheus registry at `/metrics` endpoint for scraping
//! and provides the HTTP-request observability middleware applied to every
//! mounted route (jsonrpc / ws / sse / admin).
//!
//! # Sync Metrics Integration
//!
//! Sync-protocol metrics are registered in `crates/node/src/run.rs::start`
//! via `PrometheusSyncMetrics::new(&mut registry)` before the server is
//! started. This module exposes:
//! - `merod_build_info{version, peer_id}` — constant-1 beacon (registered
//!   by node-side metrics)
//! - `http_requests_total{method, path, status}` — per-request counter
//! - `http_request_duration_seconds{method, path, status}` — per-request
//!   latency histogram
//!
//! `path` is the matched axum route template (e.g. `/jsonrpc`), never the
//! raw URI — embedded IDs would blow up cardinality. `status` is the
//! response code class (`2xx` / `4xx` / `5xx`).

use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::MatchedPath;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::routing::{get, Router};
use axum::Extension;
use prometheus_client::encoding::text::encode;
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;
use tracing::{debug, info};

use crate::config::ServerConfig;

/// HTTP request labels for the middleware. See module-level docs for
/// cardinality discipline.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct HttpLabels {
    pub method: String,
    pub path: String,
    pub status: String,
}

/// Recording handles for HTTP-request middleware. Cheap to clone — both
/// `Family` instances are `Arc<…>` under the hood.
#[derive(Clone, Debug)]
pub struct HttpMetrics {
    pub requests_total: Family<HttpLabels, Counter>,
    pub duration_seconds: Family<HttpLabels, Histogram>,
}

impl HttpMetrics {
    /// Register the two HTTP families against `registry`.
    pub fn new(registry: &mut Registry) -> Self {
        let requests_total: Family<HttpLabels, Counter> = Family::default();
        registry.register(
            "http_requests_total",
            "Total HTTP requests served by the merod server, by method / \
             matched path / status class",
            requests_total.clone(),
        );
        // Buckets cover the realistic merod-server latency range: 1ms →
        // 32s. Endpoints under 1ms compress into the first bucket; the
        // 32s tail catches handler hangs without saturating prematurely.
        let duration_seconds: Family<HttpLabels, Histogram> =
            Family::new_with_constructor(|| Histogram::new(exponential_buckets(0.001, 2.0, 15)));
        registry.register(
            "http_request_duration_seconds",
            "End-to-end HTTP request duration in seconds",
            duration_seconds.clone(),
        );
        Self {
            requests_total,
            duration_seconds,
        }
    }
}

/// Axum middleware that records request latency + count per matched route.
///
/// Inserts itself between auth/CORS layers and the route handlers. Uses
/// [`MatchedPath`] to extract the route template — falls back to `unknown`
/// when no template is available (the request didn't match any nested
/// route, typically 404s on unknown URIs).
pub async fn track_request(
    Extension(metrics): Extension<HttpMetrics>,
    req: Request<Body>,
    next: Next,
) -> axum::response::Response {
    let method = req.method().clone();
    let matched_path = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "unknown".to_owned());
    let start = Instant::now();

    let response = next.run(req).await;

    let elapsed = start.elapsed().as_secs_f64();
    let status = response.status();
    let labels = HttpLabels {
        method: method.as_str().to_owned(),
        path: matched_path,
        status: status_class(status),
    };
    metrics.requests_total.get_or_create(&labels).inc();
    metrics
        .duration_seconds
        .get_or_create(&labels)
        .observe(elapsed);
    response
}

/// Collapse a status code into a low-cardinality class label. `1xx`/`2xx`/
/// `3xx`/`4xx`/`5xx`/`unknown`. Raw status codes would create a label
/// value per code; the class gives operators ~all the signal they need
/// without paying per-code storage.
fn status_class(status: StatusCode) -> String {
    match status.as_u16() {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "unknown",
    }
    .to_owned()
}

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
    // Operators triaging "VictoriaMetrics shows only `up`" need to distinguish
    // "scrape reaches us but body is empty" from "scrape never reaches us."
    // This log fires once per scrape with the response size, so a Docker-logs
    // grep on a node under test surfaces both signals.
    debug!(bytes = buffer.len(), "metrics scrape served");
    buffer
}
