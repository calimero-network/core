use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::SetTeeAuthoringPolicyRequest;
use calimero_server_primitives::admin::SetTeeAuthoringPolicyApiResponse;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// `DELETE …/settings/tee-authoring-policy`: turn TEE authorship off in the
/// namespace.
///
/// The same op as a `PUT` with an empty list, so the two can never disagree
/// about what "off" means: every TEE loses its authority, and admitted TEEs stay
/// members.
pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Turning TEE authorship off");

    let result = state
        .ctx_client
        .set_tee_authoring_policy(SetTeeAuthoringPolicyRequest {
            group_id,
            allowed_mrtd: Vec::new(),
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => ApiResponse {
            payload: SetTeeAuthoringPolicyApiResponse {},
        }
        .into_response(),
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to turn TEE authorship off");
            err.into_response()
        }
    }
}
