use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::LeaveGroupRequest;
use calimero_server_primitives::admin::{LeaveGroupApiResponse, LeaveGroupApiResponseData};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let auth_caller = auth_key.map(|Extension(k)| k.0);

    info!(
        group_id=%group_id_str,
        ?auth_caller,
        "Leaving group (publishing MemberLeft)"
    );

    let result = state
        .ctx_client
        .leave_group(LeaveGroupRequest { group_id })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(
                group_id=%group_id_str,
                member=%resp.member_public_key,
                "Successfully left group"
            );
            ApiResponse {
                payload: LeaveGroupApiResponse {
                    data: LeaveGroupApiResponseData {
                        group_id: hex::encode(resp.group_id.to_bytes()),
                        member_public_key: resp.member_public_key,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to leave group");
            err.into_response()
        }
    }
}
