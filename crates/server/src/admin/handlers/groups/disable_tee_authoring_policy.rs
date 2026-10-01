use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_context_client::group::SetTeeAuthoringPolicyRequest;
use calimero_server_primitives::admin::{
    DisableTeeAuthoringPolicyApiRequest, SetTeeAuthoringPolicyApiResponse,
};
use calimero_server_primitives::validation::Validate;
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
///
/// The body is optional: `{"rootProof": "…"}` carries the signing admin's root
/// proof for the empty policy, and a bare `DELETE` has this node sign it when it
/// holds that root.
pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    body: Option<Json<DisableTeeAuthoringPolicyApiRequest>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    let req = body.map(|Json(req)| req).unwrap_or_default();
    if let Some(error) = req.validate().into_iter().next() {
        return crate::admin::service::ApiError {
            status_code: axum::http::StatusCode::BAD_REQUEST,
            message: error.to_string(),
        }
        .into_response();
    }
    let root_proof = match crate::admin::handlers::root_proof::decode(req.root_proof.as_deref()) {
        Ok(proof) => proof,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Turning TEE authorship off");

    let result = state
        .ctx_client
        .set_tee_authoring_policy(SetTeeAuthoringPolicyRequest {
            group_id,
            allowed_mrtd: Vec::new(),
            root_proof,
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
