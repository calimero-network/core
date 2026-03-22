use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::RetryGroupUpgradeRequest;
use calimero_server_primitives::admin::{
    RetryGroupUpgradeApiRequest, UpgradeGroupApiResponse, UpgradeGroupApiResponseData,
};
use tracing::{error, info};

use super::parse_group_id;
use super::upgrade_group::format_status;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<RetryGroupUpgradeApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Retrying group upgrade");

    let result = state
        .ctx_client
        .retry_group_upgrade(RetryGroupUpgradeRequest {
            group_id,
            requester: auth_key.map(|Extension(k)| k.0).or(req.requester),
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            let (status_str, total, completed, failed) = format_status(&resp.status);
            info!(group_id=%group_id_str, "Group upgrade retry initiated");
            ApiResponse {
                payload: UpgradeGroupApiResponse {
                    data: UpgradeGroupApiResponseData {
                        group_id: hex::encode(resp.group_id.to_bytes()),
                        status: status_str,
                        total,
                        completed,
                        failed,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to retry group upgrade");
            err.into_response()
        }
    }
}
