use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::GetMemberCapabilitiesRequest;
use calimero_server_primitives::admin::{
    GetMemberCapabilitiesApiData, GetMemberCapabilitiesApiResponse,
};
use tracing::{error, info};

use super::{parse_group_id, parse_identity};
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path((group_id_str, identity_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let member = match parse_identity(&identity_str) {
        Ok(pk) => pk,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, identity=%identity_str, "Getting member capabilities");

    let result = state
        .ctx_client
        .get_member_capabilities(GetMemberCapabilitiesRequest { group_id, member })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(response) => {
            info!(group_id=%group_id_str, identity=%identity_str, "Got member capabilities");
            ApiResponse {
                payload: GetMemberCapabilitiesApiResponse {
                    data: GetMemberCapabilitiesApiData {
                        capabilities: response.capabilities,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, identity=%identity_str, error=?err, "Failed to get member capabilities");
            err.into_response()
        }
    }
}
