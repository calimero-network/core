use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::JoinContextRequest;
use calimero_server_primitives::admin::{JoinContextApiResponse, JoinContextApiResponseData};
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

    info!(context_id=%context_id_str, "Joining context via group membership");

    let result = state
        .ctx_client
        .join_context(JoinContextRequest { context_id })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(
                context_id=%resp.context_id,
                member=%resp.member_public_key,
                "Successfully joined context via group"
            );
            ApiResponse {
                payload: JoinContextApiResponse {
                    data: JoinContextApiResponseData {
                        context_id: resp.context_id,
                        member_public_key: resp.member_public_key,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(context_id=%context_id_str, error=?err, "Failed to join context via group");
            err.into_response()
        }
    }
}
