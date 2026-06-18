use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::{GetMemberMetadataRequest, SetMemberMetadataRequest};
use calimero_server_primitives::admin::{
    GetMetadataApiResponse, SetMemberMetadataApiRequest, SetMetadataApiResponse,
};
use tracing::{error, info};

use super::{parse_group_id, parse_identity};
use crate::admin::handlers::requester::resolve_requester;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path((group_id_str, identity_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<SetMemberMetadataApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let member = match parse_identity(&identity_str) {
        Ok(pk) => pk,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, identity=%identity_str, "Setting member metadata");

    let requester = match resolve_requester(auth_key, req.requester) {
        Ok(r) => r,
        Err(err) => return err.into_response(),
    };

    let result = state
        .ctx_client
        .set_member_metadata(SetMemberMetadataRequest {
            group_id,
            member,
            name: req.name,
            data: req.data,
            requester,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, identity=%identity_str, "Member metadata set");
            ApiResponse {
                payload: SetMetadataApiResponse {},
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, identity=%identity_str, error=?err, "Failed to set member metadata");
            err.into_response()
        }
    }
}

pub async fn get_handler(
    Path((group_id_str, identity_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    let member = match parse_identity(&identity_str) {
        Ok(pk) => pk,
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
