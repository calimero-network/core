use axum::extract::State;
use axum::response::IntoResponse;
use calimero_primitives::application::ApplicationId;
use calimero_store::Store;
use serde::Serialize;
use tower_sessions::Session;

use super::add_client_key::parse_api_error;
use crate::admin::service::ApiResponse;
use crate::admin::storage::did::{get_or_create_did, Did};
use crate::APPLICATION_ID;

#[derive(Debug, Serialize)]
struct DidResponse {
    data: Did,
}

pub async fn fetch_did_handler(_session: Session, State(store): State<Store>) -> impl IntoResponse {
    let application_id = ApplicationId(APPLICATION_ID.to_string());

    let did = get_or_create_did(application_id, &store).map_err(|err| parse_api_error(err));

    return match did {
        Ok(did) => ApiResponse {
            payload: DidResponse { data: did },
        }
        .into_response(),
        Err(err) => err.into_response(),
    };
}
