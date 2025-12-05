use std::sync::Arc;
use std::time::Instant;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::IntoResponse;

use crate::metrics::RelayerMetrics;

/// RAII guard to ensure http_requests_active is decremented even on panic
struct ActiveRequestGuard {
    metrics: Arc<RelayerMetrics>,
}

impl ActiveRequestGuard {
    fn new(metrics: Arc<RelayerMetrics>) -> Self {
        metrics.inc_http_active();
        Self { metrics }
    }
}

impl Drop for ActiveRequestGuard {
    fn drop(&mut self) {
        self.metrics.dec_http_active();
    }
}

/// Normalize HTTP method to prevent cardinality explosion
/// Only standard HTTP methods are tracked individually, others are aggregated
fn normalize_http_method(method: &axum::http::Method) -> &'static str {
    if method == axum::http::Method::GET {
        "GET"
    } else if method == axum::http::Method::POST {
        "POST"
    } else if method == axum::http::Method::PUT {
        "PUT"
    } else if method == axum::http::Method::DELETE {
        "DELETE"
    } else if method == axum::http::Method::PATCH {
        "PATCH"
    } else if method == axum::http::Method::HEAD {
        "HEAD"
    } else if method == axum::http::Method::OPTIONS {
        "OPTIONS"
    } else {
        "OTHER"
    }
}

pub async fn track_metrics(
    metrics: Option<Arc<RelayerMetrics>>,
    req: Request,
    next: Next,
) -> impl IntoResponse {
    let Some(metrics) = metrics else {
        // No metrics available, just pass through
        return next.run(req).await;
    };

    let raw_path = req.uri().path();

    // Skip tracking for observability endpoints to avoid self-referential metrics
    if raw_path == "/metrics" || raw_path == "/health" {
        return next.run(req).await;
    }

    // Normalize path to prevent cardinality explosion
    // Only track known paths, aggregate everything else
    let path = match raw_path {
        "/" => "/",
        _ => "other",
    };

    let start = Instant::now();
    // Normalize HTTP method to prevent cardinality explosion
    let method = normalize_http_method(req.method());

    // Use RAII guard to ensure active count is decremented even on panic/cancellation
    let _active_guard = ActiveRequestGuard::new(metrics.clone());

    // Track request size, use 0 if content-length header is missing
    let request_size = req
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    metrics.record_http_request_size(method, path, request_size as f64);

    let response = next.run(req).await;

    // Guard will decrement active count when dropped (even on panic)

    let status_code = response.status().as_u16().to_string();
    let duration = start.elapsed();

    metrics.record_http_request(method, path, &status_code, duration.as_secs_f64());

    // Track response size, use 0 if content-length header is missing
    let response_size = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    metrics.record_http_response_size(method, path, &status_code, response_size as f64);

    response
}
