use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::{GetMemberMetadataRequest, SetMemberMetadataRequest};
use calimero_server_primitives::admin::{
    GetMetadataApiResponse, SetMemberMetadataApiRequest, SetMetadataApiResponse,
};
use tracing::{error, info};

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

    info!(group_id=%group_id_str, identity=%account_str, "Setting member metadata");

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
            info!(group_id=%group_id_str, identity=%account_str, "Member metadata set");
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
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
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
