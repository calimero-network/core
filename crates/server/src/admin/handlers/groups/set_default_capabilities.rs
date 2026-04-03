use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::SetDefaultCapabilitiesRequest;
use calimero_server_primitives::admin::SetDefaultCapabilitiesApiRequest;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse, Empty};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<SetDefaultCapabilitiesApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, default_capabilities=req.default_capabilities, "Setting default capabilities");

    let result = state
        .ctx_client
        .set_default_capabilities(SetDefaultCapabilitiesRequest {
            group_id,
            default_capabilities: req.default_capabilities,
            requester: auth_key.map(|Extension(k)| k.0).or(req.requester),
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, "Default capabilities updated");
            ApiResponse { payload: Empty }.into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to set default capabilities");
            err.into_response()
        }
    }
}
