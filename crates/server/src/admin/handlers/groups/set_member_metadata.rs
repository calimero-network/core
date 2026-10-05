use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::{GetMemberMetadataRequest, SetMemberMetadataRequest};
use calimero_server_primitives::admin::{
    GetMetadataApiResponse, SetMemberMetadataApiRequest, SetMetadataApiResponse,
};
use tracing::{debug, error};

use super::{parse_account, parse_group_id};
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path((group_id_str, account_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<SetMemberMetadataApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let member = match parse_account(&account_str) {
        Ok(account) => account,
        Err(err) => return err.into_response(),
    };

    debug!(group_id=%group_id_str, identity=%account_str, "Setting member metadata");

    let result = state
        .ctx_client
        .set_member_metadata(SetMemberMetadataRequest {
            group_id,
            member,
            name: req.name,
            data: req.data,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            debug!(group_id=%group_id_str, identity=%account_str, "Member metadata set");
            ApiResponse {
                payload: SetMetadataApiResponse {},
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, identity=%account_str, error=?err, "Failed to set member metadata");
            err.into_response()
        }
    }
}

pub async fn get_handler(
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
    // `group:list-own` (an account reads its own display name through it), so
    // it must be confined to its own groups here, as the capabilities read is.
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

    match state
        .ctx_client
        .get_member_metadata(GetMemberMetadataRequest { group_id, member })
        .await
        .map_err(parse_api_error)
    {
        Ok(record) => ApiResponse {
            payload: GetMetadataApiResponse { data: record },
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
