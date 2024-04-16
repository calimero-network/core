use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use serde::Serialize;
use tower_sessions::Session;

use super::add_client_key::parse_api_error;
use crate::admin::service::{AdminState, ApiResponse};
use crate::admin::storage::did::{get_or_create_did, Did};

#[derive(Debug, Serialize)]
struct DidResponse {
    data: Did,
}

pub async fn fetch_did_handler(
    _session: Session,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let did = get_or_create_did(&state.store).map_err(|err| parse_api_error(err));
    return match did {
        Ok(did) => ApiResponse {
            payload: DidResponse { data: did },
        }
        .into_response(),
        Err(err) => err.into_response(),
    };
}
