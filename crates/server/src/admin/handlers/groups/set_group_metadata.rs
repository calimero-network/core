use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::{GetGroupMetadataRequest, SetGroupMetadataRequest};
use calimero_server_primitives::admin::{
    GetMetadataApiResponse, SetGroupMetadataApiRequest, SetMetadataApiResponse,
};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<SetGroupMetadataApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Setting group metadata");

    let result = state
        .ctx_client
        .set_group_metadata(SetGroupMetadataRequest {
            group_id,
            name: req.name,
            data: req.data,
            // Authenticated key (when present) wins over an explicit
            // `requester` in the body — the body field is only honored for
            // unauthenticated / local calls.
            requester: auth_key.map(|Extension(k)| k.0).or(req.requester),
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, "Group metadata set");
            ApiResponse {
                payload: SetMetadataApiResponse {},
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to set group metadata");
            err.into_response()
        }
    }
}

pub async fn get_handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    match state
        .ctx_client
        .get_group_metadata(GetGroupMetadataRequest { group_id })
        .await
        .map_err(parse_api_error)
    {
        Ok(record) => ApiResponse {
            payload: GetMetadataApiResponse { data: record },
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
