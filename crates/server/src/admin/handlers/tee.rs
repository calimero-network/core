use axum::routing::{get, post};
use axum::Router;

mod attestation;
mod info;

pub fn service() -> Router {
    Router::new()
        .route("/info", get(info::handler))
        .route("/attest", post(attestation::handler))
}
