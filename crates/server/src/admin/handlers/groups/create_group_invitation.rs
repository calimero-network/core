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
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<CreateGroupInvitationApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Creating group invitation");

    // Prefer the authenticated identity over the caller-supplied requester to
    // prevent authorization bypass via a spoofed public key in the request body.
    let requester = auth_key.map(|Extension(k)| k.0).or(req.requester);

    let result = state
        .ctx_client
        .create_group_invitation(CreateGroupInvitationRequest {
            group_id,
            requester,
            expiration_block_height: req.expiration_block_height,
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
