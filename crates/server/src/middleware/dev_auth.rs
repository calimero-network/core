use std::sync::Arc;

use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use axum::Extension;

use crate::admin::service::AdminState;

pub async fn dev_mode_auth(
    state: Extension<Arc<AdminState>>,
    request: Request<axum::body::Body>,
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
        .and_then(|v| Some(v.as_bytes()))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !public_key.verify(timestamp, &signature) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let response = next.run(request).await;

    Ok(response)
}
