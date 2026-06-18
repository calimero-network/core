use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::SyncGroupRequest;
use calimero_server_primitives::admin::{
    SyncGroupApiRequest, SyncGroupApiResponse, SyncGroupApiResponseData,
};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::requester::resolve_requester;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<SyncGroupApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Syncing group state from contract");

    let requester = match resolve_requester(auth_key, req.requester) {
        Ok(r) => r,
        Err(err) => return err.into_response(),
    };

    let result = state
        .ctx_client
        .sync_group(SyncGroupRequest {
            group_id,
            requester,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(group_id=%group_id_str, "Group state synced successfully");
            ApiResponse {
                payload: SyncGroupApiResponse {
                    data: SyncGroupApiResponseData {
                        group_id: hex::encode(resp.group_id.to_bytes()),
                        app_key: hex::encode(resp.app_key),
                        target_application_id: resp.target_application_id,
                        member_count: resp.member_count,
                        context_count: resp.context_count,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to sync group state");
            err.into_response()
        }
    }
}
