use axum::routing::{get, post};
use axum::Router;

mod attest;
mod info;
mod verify_quote;

pub fn service() -> Router {
    Router::new()
        .route("/info", get(info::handler))
        .route("/attest", post(attest::handler))
        .route("/verify-quote", post(verify_quote::handler))
}
