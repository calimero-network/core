use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use axum::Extension;
use chrono::{Duration, TimeZone, Utc};

use crate::AdminState;

const TIMESTAMP_THRESHOLD: i64 = 5;

pub async fn dev_mode_auth(
    state: Extension<Arc<AdminState>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let public_key = &state.keypair.public();

    let signature = request
        .headers()
        .get("X-Signature")
        .and_then(|v| bs58::decode(v).into_vec().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let timestamp = request
        .headers()
        .get("X-Timestamp")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
        .and_then(|t| Utc.timestamp_opt(t, 0).single())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let now = Utc::now();

    let time_diff = now.signed_duration_since(timestamp);
    if time_diff > Duration::seconds(TIMESTAMP_THRESHOLD)
        || time_diff < Duration::seconds(-TIMESTAMP_THRESHOLD)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let message = timestamp.timestamp().to_string();

    if !public_key.verify(message.as_bytes(), &signature) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let response = next.run(request).await;

    Ok(response)
}
