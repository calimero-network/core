use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::JoinGroupRequest;
use calimero_server_primitives::admin::{
    JoinGroupApiRequest, JoinGroupApiResponse, JoinGroupApiResponseData,
};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<JoinGroupApiRequest>,
) -> impl IntoResponse {
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
        .map_err(|err| {
            // The failure users actually hit here is peer discovery: the
            // joiner holds a valid invitation but cannot open a stream to
            // any current member within the discovery deadline (see
            // node/src/sync/manager/namespace_join.rs). parse_api_error
            // deliberately masks untyped reports as a bare 500, which the
            // client then can't distinguish from a real internal fault —
            // so type this one as a retryable 504 with an actionable
            // message before falling through.
            if format!("{err:?}").contains("namespace-join stream") {
                ApiError {
                    status_code: StatusCode::GATEWAY_TIMEOUT,
                    message: "could not reach any member of this namespace to \
                              complete the join; make sure the inviting node is \
                              online and retry"
                        .into(),
                }
            } else {
                parse_api_error(err)
            }
        });

    match result {
        Ok(resp) => {
            let group_id_hex = hex::encode(resp.group_id.to_bytes());
            info!(group_id=%group_id_hex, member=%resp.member_identity, "Joined namespace successfully");
            ApiResponse {
                payload: JoinGroupApiResponse {
                    data: JoinGroupApiResponseData {
                        group_id: group_id_hex,
                        member_identity: resp.member_identity,
                        governance_op: hex::encode(&resp.governance_op_bytes),
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(namespace_id=%namespace_id_str, error=?err, "Failed to join namespace");
            err.into_response()
        }
    }
}
