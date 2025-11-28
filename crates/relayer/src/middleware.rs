use std::sync::Arc;
use std::time::Instant;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::IntoResponse;

use crate::metrics::RelayerMetrics;

pub async fn track_metrics(
    metrics: Arc<RelayerMetrics>,
    req: Request,
    next: Next,
) -> impl IntoResponse {
    let start = Instant::now();
    let method = req.method().to_string();
    let path = req.uri().path().to_string();

    metrics.inc_http_active();

    if let Some(content_length) = req.headers().get("content-length") {
        if let Ok(size_str) = content_length.to_str() {
            if let Ok(size) = size_str.parse::<u64>() {
                metrics.record_http_request_size(&method, &path, size as f64);
            }
        }
    }

    let response = next.run(req).await;

    metrics.dec_http_active();

    let status_code = response.status().as_u16().to_string();
    let duration = start.elapsed();

    metrics.record_http_request(&method, &path, &status_code, duration.as_secs_f64());

    if let Some(content_length) = response.headers().get("content-length") {
        if let Ok(size_str) = content_length.to_str() {
            if let Ok(size) = size_str.parse::<u64>() {
                metrics.record_http_response_size(&method, &path, &status_code, size as f64);
            }
        }
    }

    response
}
