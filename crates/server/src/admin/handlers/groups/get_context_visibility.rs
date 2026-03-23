use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::GetContextVisibilityRequest;
use calimero_server_primitives::admin::{
    GetContextVisibilityApiData, GetContextVisibilityApiResponse,
};
use tracing::{error, info};

use super::{parse_context_id, parse_group_id};
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path((group_id_str, context_id_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let context_id = match parse_context_id(&context_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, context_id=%context_id_str, "Getting context visibility");

    let result = state
        .ctx_client
        .get_context_visibility(GetContextVisibilityRequest {
            group_id,
            context_id,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(response) => {
            let mode_str = match response.mode {
                calimero_context_config::VisibilityMode::Open => "open",
                calimero_context_config::VisibilityMode::Restricted => "restricted",
            };

            info!(group_id=%group_id_str, context_id=%context_id_str, "Got context visibility");
            ApiResponse {
                payload: GetContextVisibilityApiResponse {
                    data: GetContextVisibilityApiData {
                        mode: mode_str.to_owned(),
                        creator: response.creator,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, context_id=%context_id_str, error=?err, "Failed to get context visibility");
            err.into_response()
        }
    }
}
