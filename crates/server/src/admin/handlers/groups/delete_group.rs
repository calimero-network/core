use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::DeleteGroupRequest;
use calimero_server_primitives::admin::{
    DeleteGroupApiRequest, DeleteGroupApiResponse, DeleteGroupApiResponseData,
};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<DeleteGroupApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Deleting group");

    let result = state
        .ctx_client
        .delete_group(DeleteGroupRequest {
            group_id,
            requester: req.requester,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(response) => {
            info!(group_id=%group_id_str, deleted=%response.deleted, "Group deletion completed");
            ApiResponse {
                payload: DeleteGroupApiResponse {
                    data: DeleteGroupApiResponseData {
                        is_deleted: response.deleted,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to delete group");
            err.into_response()
        }
    }
}
