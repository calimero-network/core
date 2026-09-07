use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::CreateGroupInvitationRequest;
use calimero_server_primitives::admin::{
    CreateGroupInvitationApiRequest, CreateGroupInvitationApiResponse,
    CreateGroupInvitationApiResponseData,
};
use tracing::{error, info};

use super::parse_group_id;
use axum::http::StatusCode;

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<CreateGroupInvitationApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    // Parsed before anything is signed: an admitter the caller cannot spell is a
    // restriction that would silently not apply, and this route previously
    // accepted the field and dropped it, which is worse than refusing it.
    let admitters = match req
        .admitters
        .iter()
        .map(|a| a.parse::<calimero_account::AccountId>())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(list) => list,
        Err(_) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "admitters must each be 64 hex characters (32 bytes)".to_owned(),
            }
            .into_response();
        }
    };

    info!(group_id=%group_id_str, "Creating group invitation");

    let result = state
        .ctx_client
        .create_group_invitation(CreateGroupInvitationRequest {
            group_id,
            expiration_timestamp: req.expiration_timestamp,
            admitters,
            admitter_addrs: req.admitter_addrs,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(group_id=%group_id_str, "Group invitation created");
            ApiResponse {
                payload: CreateGroupInvitationApiResponse {
                    data: CreateGroupInvitationApiResponseData {
                        invitation: resp.invitation,
                        group_name: resp.group_name,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to create group invitation");
            err.into_response()
        }
    }
}
