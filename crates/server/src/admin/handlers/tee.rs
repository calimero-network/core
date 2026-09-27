use axum::routing::{get, post};
use axum::Router;

mod announce;
mod attest;
mod collateral;
pub mod evidence_retry;
pub mod fleet_join;
mod info;
mod registration_attest;

pub fn service() -> Router {
    Router::new()
        .route("/info", get(info::handler))
        .route("/attest", post(attest::handler))
}

pub fn protected_service() -> Router {
    Router::new()
        .route("/fleet-join", post(fleet_join::handler))
        .route("/registration-attest", post(registration_attest::handler))
}
