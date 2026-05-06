use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::LeaveContextRequest;
use calimero_server_primitives::admin::{LeaveContextApiResponse, LeaveContextApiResponseData};
use tracing::{error, info};

use crate::admin::handlers::groups::parse_context_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(context_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let context_id = match parse_context_id(&context_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(context_id=%context_id_str, "Leaving context locally (no DAG op published)");

    let result = state
        .ctx_client
        .leave_context(LeaveContextRequest { context_id })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(
                context_id=%resp.context_id,
                member=%resp.member_public_key,
                "Successfully left context locally"
            );
            ApiResponse {
                payload: LeaveContextApiResponse {
                    data: LeaveContextApiResponseData {
                        context_id: resp.context_id,
                        member_public_key: resp.member_public_key,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(context_id=%context_id_str, error=?err, "Failed to leave context");
            err.into_response()
        }
    }
}
