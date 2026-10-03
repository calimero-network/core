use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::GetCascadeStatusRequest;
use calimero_server_primitives::admin::{CascadeStatusApiEntry, GetCascadeStatusApiResponse};
use tracing::{debug, error, info};

use super::{parse_group_id, upgrade_info_to_api_data};
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<crate::auth::AuthenticatedNodeOwner>>,
    account: Option<Extension<crate::auth::AuthenticatedAccount>>,
    device: Option<Extension<crate::auth::AuthenticatedDevice>>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    // Before any read: a delegated session reaches this route on the narrow
    // `group:list-own`, so it must be confined to its own groups here.
    if let Some(refusal) = crate::admin::caller_scope::refuse_group_outside_caller_scope(
        &state.ctx_client,
        node_owner,
        account,
        device,
        &namespace_id,
    ) {
        return refusal;
    }

    info!(namespace_id=%namespace_id_str, "Getting cascade status");

    let result = state
        .ctx_client
        .get_cascade_status(GetCascadeStatusRequest { namespace_id })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(entries) => {
            let data = entries
                .into_iter()
                .map(|e| CascadeStatusApiEntry {
                    group_id: hex::encode(e.group_id.to_bytes()),
                    upgrade: upgrade_info_to_api_data(&e.upgrade),
                    cascade_hlc: e.cascade_hlc.map(|ts| ts.to_string()),
                })
                .collect();

            ApiResponse {
                payload: GetCascadeStatusApiResponse { data },
            }
            .into_response()
        }
        Err(err) => {
            // A 4xx is the caller naming something absent or not theirs, not a
            // fault of this node. On a fleet node these journals ship to a
            // central store, where an `ERROR` per poll buries real faults.
            if err.is_client_fault() {
                debug!(namespace_id=%namespace_id_str, error=?err, "Failed to get cascade status");
            } else {
                error!(namespace_id=%namespace_id_str, error=?err, "Failed to get cascade status");
            }
            err.into_response()
        }
    }
}
