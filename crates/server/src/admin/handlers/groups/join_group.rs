use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::JoinGroupRequest;
use calimero_server_primitives::admin::{
    JoinGroupApiRequest, JoinGroupApiResponse, JoinGroupApiResponseData,
};
use tracing::{debug, error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    client_key: Option<Extension<crate::auth::ClientKeyScope>>,
    ValidatedJson(req): ValidatedJson<JoinGroupApiRequest>,
) -> impl IntoResponse {
    if let Some(refusal) = crate::admin::client_key_scope::refuse_join_outside_binding(
        &state,
        client_key.as_deref(),
        &req.invitation,
    ) {
        return refusal;
    }

    info!("Joining group via invitation");

    let result = state
        .ctx_client
        .join_group(JoinGroupRequest {
            invitation: req.invitation,
            group_name: req.group_name,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            let group_id_hex = hex::encode(resp.group_id.to_bytes());
            debug!(group_id=%group_id_hex, member=%resp.member_identity, "Joined group successfully");
            ApiResponse {
                payload: JoinGroupApiResponse {
                    data: JoinGroupApiResponseData {
                        group_id: group_id_hex,
                        member_identity: resp.member_identity,
                        member_account: resp.member_account.to_string(),
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, "Failed to join group");
            err.into_response()
        }
    }
}
