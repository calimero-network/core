use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::GetMemberCapabilitiesRequest;
use calimero_server_primitives::admin::{
    GetMemberCapabilitiesApiData, GetMemberCapabilitiesApiResponse,
};
use tracing::{debug, error};

use super::{parse_account, parse_group_id};
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path((group_id_str, account_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<crate::auth::AuthenticatedNodeOwner>>,
    account: Option<Extension<crate::auth::AuthenticatedAccount>>,
    device: Option<Extension<crate::auth::AuthenticatedDevice>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
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
        &group_id,
    ) {
        return refusal;
    }

    let member = match parse_account(&account_str) {
        Ok(account) => account,
        Err(err) => return err.into_response(),
    };

    debug!(group_id=%group_id_str, identity=%account_str, "Getting member capabilities");

    let result = state
        .ctx_client
        .get_member_capabilities(GetMemberCapabilitiesRequest { group_id, member })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(response) => {
            debug!(group_id=%group_id_str, identity=%account_str, "Got member capabilities");
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
            // A 4xx is the caller naming something absent or not theirs, not a
            // fault of this node. On a fleet node these journals ship to a
            // central store, where an `ERROR` per poll buries real faults.
            if err.is_client_fault() {
                debug!(group_id=%group_id_str, identity=%account_str, error=?err, "Failed to get member capabilities");
            } else {
                error!(group_id=%group_id_str, identity=%account_str, error=?err, "Failed to get member capabilities");
            }
            err.into_response()
        }
    }
}
