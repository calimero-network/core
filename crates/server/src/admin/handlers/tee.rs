use axum::routing::{get, post};
use axum::Router;

mod attest;
pub(crate) mod collateral;
pub mod evidence_retry;
pub mod fleet_join;
mod info;
mod prompt;
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
