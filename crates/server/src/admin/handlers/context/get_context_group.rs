use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::GetGroupForContextRequest;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GetContextGroupApiResponse;
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::caller_scope::{admits_context, list_scope_for};
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::auth::{AuthenticatedAccount, AuthenticatedDevice, AuthenticatedNodeOwner};
use crate::AdminState;

pub async fn handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<AuthenticatedNodeOwner>>,
    account: Option<Extension<AuthenticatedAccount>>,
    device: Option<Extension<AuthenticatedDevice>>,
) -> impl IntoResponse {
    info!(%context_id, "Getting group for context");

    // This route answers with the very id the scope check looks up, so an
    // unscoped version is the sharpest of the four: it turns a guessed context
    // id into the group that owns it, which is the link an outsider needs to
    // work out who else is on this relay.
    //
    // Same predicate as `GET /admin-api/contexts/:id`, resolved per request. A
    // node owner, and a node running with no auth guard, get `NodeWide` and are
    // unaffected.
    let scope = match list_scope_for(&state.ctx_client, node_owner, account, device) {
        Ok(scope) => scope,
        Err(err) => {
            error!(%context_id, error=?err, "Failed to resolve the caller's list scope");
            return parse_api_error(err).into_response();
        }
    };

    match admits_context(state.ctx_client.datastore(), &context_id, &scope) {
        Ok(true) => {}
        Ok(false) => {
            info!(%context_id, "Refusing context group: caller is not a member of this context's group");
            return ApiError {
                status_code: StatusCode::FORBIDDEN,
                message: "account is not a member of the group owning this context".to_owned(),
            }
            .into_response();
        }
        Err(err) => {
            error!(%context_id, error=?err, "Failed to resolve the context's group");
            return parse_api_error(err).into_response();
        }
    }

    let result = state
        .ctx_client
        .get_group_for_context(GetGroupForContextRequest { context_id })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(group_id) => {
            info!(%context_id, "Context group retrieved successfully");
            ApiResponse {
                payload: GetContextGroupApiResponse {
                    data: group_id.map(|g| hex::encode(g.to_bytes())),
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(%context_id, error=?err, "Failed to get group for context");
            err.into_response()
        }
    }
}
