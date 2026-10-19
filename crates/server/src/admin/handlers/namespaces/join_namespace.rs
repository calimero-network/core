use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::JoinGroupRequest;
use calimero_server_primitives::admin::{
    JoinGroupApiRequest, JoinNamespaceApiResponse, JoinNamespaceApiResponseData,
};
use reqwest::StatusCode;
use tracing::{debug, error, info};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{join_refusal_code, parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    client_key: Option<Extension<crate::auth::ClientKeyScope>>,
    ValidatedJson(req): ValidatedJson<JoinGroupApiRequest>,
) -> impl IntoResponse {
    if let Some(refusal) = crate::admin::client_key_scope::refuse_join_outside_binding(
        &state,
        client_key.as_deref(),
        &req.invitation,
    ) {
        return refusal;
    }

    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let invitation_group_id = req.invitation.invitation.group_id;
    if invitation_group_id != namespace_id {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "invitation group_id does not match namespace_id in path".into(),
        }
        .into_response();
    }

    info!(namespace_id=%namespace_id_str, "Joining namespace via invitation");

    let result = state
        .ctx_client
        .join_group(JoinGroupRequest {
            invitation: req.invitation,
            group_name: req.group_name,
        })
        .await
        // A join nobody could endorse is typed (`ContextError::JoinNotEndorsed`)
        // and mapped by `parse_api_error` to a retryable status, for this route
        // and `POST /groups/join` alike. `code` names which refusal it was.
        .map_err(|err| {
            let code = join_refusal_code(&err);
            (parse_api_error(err), code)
        });

    match result {
        Ok(resp) => {
            let namespace_id_hex = hex::encode(resp.group_id.to_bytes());
            debug!(namespace_id=%namespace_id_hex, member=%resp.member_identity, "Joined namespace successfully");
            ApiResponse {
                payload: JoinNamespaceApiResponse {
                    data: JoinNamespaceApiResponseData {
                        namespace_id: namespace_id_hex,
                        member_identity: resp.member_identity,
                        member_account: resp.member_account.to_string(),
                    },
                },
            }
            .into_response()
        }
        Err((err, code)) => {
            error!(namespace_id=%namespace_id_str, error=?err, ?code, "Failed to join namespace");
            err.into_coded_response(code)
        }
    }
}
