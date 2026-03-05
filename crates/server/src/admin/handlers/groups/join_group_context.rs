use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::JoinGroupContextRequest;
use calimero_server_primitives::admin::{
    JoinGroupContextApiRequest, JoinGroupContextApiResponse, JoinGroupContextApiResponseData,
};
use tracing::{error, info, warn};

use super::{decode_signing_key, parse_group_id};
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<JoinGroupContextApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    if req.requester_secret.is_some() {
        warn!("requester_secret is deprecated; register signing key via POST /admin-api/groups/:id/signing-key");
    }

    let signing_key = match req.requester_secret.as_deref().map(decode_signing_key) {
        Some(Ok(key)) => Some(key),
        Some(Err(err)) => return err.into_response(),
        None => None,
    };

    info!(group_id=%group_id_str, context_id=%req.context_id, "Joining context via group membership");

    let result = state
        .ctx_client
        .join_group_context(JoinGroupContextRequest {
            group_id,
            context_id: req.context_id,
            joiner_identity: req.joiner_identity,
            signing_key,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(
                group_id=%group_id_str,
                context_id=%resp.context_id,
                member=%resp.member_public_key,
                "Successfully joined context via group"
            );
            ApiResponse {
                payload: JoinGroupContextApiResponse {
                    data: JoinGroupContextApiResponseData {
                        context_id: resp.context_id,
                        member_public_key: resp.member_public_key,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to join context via group");
            err.into_response()
        }
    }
}
