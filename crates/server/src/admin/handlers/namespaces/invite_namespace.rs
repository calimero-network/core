use calimero_governance_store::NamespaceRepository;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::CreateGroupInvitationRequest;
use calimero_server_primitives::admin::{
    CreateGroupInvitationApiRequest, CreateGroupInvitationApiResponse,
    CreateGroupInvitationApiResponseData,
};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<CreateGroupInvitationApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    match NamespaceRepository::new(&state.store).parent(&namespace_id) {
        Ok(Some(_)) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "namespace_id must reference a root group (namespace)".into(),
            }
            .into_response();
        }
        Ok(None) => {}
        Err(err) => return parse_api_error(err).into_response(),
    }

    info!(namespace_id=%namespace_id_str, "Creating namespace invitation");

    // Parsed before anything is signed: an admitter the caller cannot spell is
    // a restriction that would silently not apply, and an invitation restricted
    // to nobody reachable is worse than an unrestricted one.
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

    let result = state
        .ctx_client
        .create_group_invitation(CreateGroupInvitationRequest {
            group_id: namespace_id,
            expiration_timestamp: req.expiration_timestamp,
            admitters,
            admitter_addrs: req.admitter_addrs,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => ApiResponse {
            payload: CreateGroupInvitationApiResponse {
                data: CreateGroupInvitationApiResponseData {
                    invitation: resp.invitation,
                    group_name: resp.group_name,
                },
            },
        }
        .into_response(),
        Err(err) => {
            error!(namespace_id=%namespace_id_str, error=?err, "Failed to create namespace invitation");
            err.into_response()
        }
    }
}
