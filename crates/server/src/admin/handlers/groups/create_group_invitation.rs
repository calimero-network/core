use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::CreateGroupInvitationRequest;
use calimero_server_primitives::admin::{
    CreateGroupInvitationApiRequest, CreateGroupInvitationApiResponse,
    CreateGroupInvitationApiResponseData,
};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
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

    info!(group_id=%group_id_str, "Creating group invitation");

    let result = state
        .ctx_client
        .create_group_invitation(CreateGroupInvitationRequest {
            group_id,
            requester: req.requester,
            invitee_identity: req.invitee_identity,
            expiration: req.expiration,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(group_id=%group_id_str, "Group invitation created");
            ApiResponse {
                payload: CreateGroupInvitationApiResponse {
                    data: CreateGroupInvitationApiResponseData {
                        payload: resp.payload.to_string(),
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
