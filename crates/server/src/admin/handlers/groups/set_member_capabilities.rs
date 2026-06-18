use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::SetMemberCapabilitiesRequest;
use calimero_server_primitives::admin::{
    SetMemberCapabilitiesApiRequest, SetMemberCapabilitiesApiResponse,
};
use tracing::{error, info};

use super::{parse_group_id, parse_identity};
use crate::admin::handlers::requester::resolve_requester;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path((group_id_str, identity_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<SetMemberCapabilitiesApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let member = match parse_identity(&identity_str) {
        Ok(pk) => pk,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, identity=%identity_str, capabilities=req.capabilities, "Setting member capabilities");

    let requester = match resolve_requester(auth_key, req.requester) {
        Ok(r) => r,
        Err(err) => return err.into_response(),
    };

    let result = state
        .ctx_client
        .set_member_capabilities(SetMemberCapabilitiesRequest {
            group_id,
            member,
            capabilities: req.capabilities,
            requester,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, identity=%identity_str, "Member capabilities updated");
            ApiResponse {
                payload: SetMemberCapabilitiesApiResponse {},
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, identity=%identity_str, error=?err, "Failed to set member capabilities");
            err.into_response()
        }
    }
}
