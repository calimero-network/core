use axum::routing::{get, post};
use axum::Router;

mod attest;
pub mod fleet_join;
mod info;
mod verify_quote;

pub fn service() -> Router {
    Router::new()
        .route("/info", get(info::handler))
        .route("/attest", post(attest::handler))
        .route("/verify-quote", post(verify_quote::handler))
}

pub fn protected_service() -> Router {
    Router::new().route("/fleet-join", post(fleet_join::handler))
}
