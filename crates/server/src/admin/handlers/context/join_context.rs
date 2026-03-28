use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{JoinContextRequest, JoinContextResponse};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(JoinContextRequest {
        invitation,
        new_member_public_key,
    }): ValidatedJson<JoinContextRequest>,
) -> impl IntoResponse {
    let result = state
        .ctx_client
        .join_context(invitation, &new_member_public_key)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(result) => {
            ApiResponse {
                payload: JoinContextResponse::new(
                    result.map(|r| (r.context_id, r.member_public_key)),
                ),
            }
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
