use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::{
    JoinSubgroupInheritanceError, JoinSubgroupInheritanceRequest,
};
use calimero_server_primitives::admin::{
    JoinSubgroupInheritanceApiResponse, JoinSubgroupInheritanceApiResponseData,
};
use reqwest::StatusCode;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Joining Open subgroup via inheritance");

    match state
        .ctx_client
        .join_subgroup_inheritance(JoinSubgroupInheritanceRequest { group_id })
        .await
    {
        Ok(resp) => {
            info!(
                group_id=%group_id_str,
                member=%resp.member_public_key,
                was_inherited=resp.was_inherited,
                "Subgroup inheritance join completed",
            );
            ApiResponse {
                payload: JoinSubgroupInheritanceApiResponse {
                    data: JoinSubgroupInheritanceApiResponseData {
                        group_id: hex::encode(resp.group_id.to_bytes()),
                        member_public_key: resp.member_public_key,
                        was_inherited: resp.was_inherited,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to join subgroup via inheritance");
            map_handler_error(err).into_response()
        }
    }
}

/// Map the actor handler's `eyre::Report` to a typed `ApiError`. Uses
/// `downcast_ref` on the carried `JoinSubgroupInheritanceError` enum so
/// the HTTP-status contract is anchored to a type, not to error-message
/// wording.
fn map_handler_error(err: eyre::Report) -> ApiError {
    if let Some(typed) = err.downcast_ref::<JoinSubgroupInheritanceError>() {
        let status_code = match typed {
            JoinSubgroupInheritanceError::GroupNotFound => StatusCode::NOT_FOUND,
            JoinSubgroupInheritanceError::NoNamespaceIdentity
            | JoinSubgroupInheritanceError::NotEligible => StatusCode::FORBIDDEN,
        };
        return ApiError {
            status_code,
            message: typed.to_string(),
        };
    }
    ApiError {
        status_code: StatusCode::INTERNAL_SERVER_ERROR,
        message: err.to_string(),
    }
}
