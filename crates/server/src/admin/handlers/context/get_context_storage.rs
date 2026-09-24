use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GetContextStorageResponse;
use tracing::{error, info};

use crate::admin::caller_scope::{admits_context, list_scope_for};
use crate::admin::handlers::usage::context_storage_bytes;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::auth::{AuthenticatedAccount, AuthenticatedDevice, AuthenticatedNodeOwner};
use crate::AdminState;

pub async fn handler(
    Path(context_id): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<AuthenticatedNodeOwner>>,
    account: Option<Extension<AuthenticatedAccount>>,
    device: Option<Extension<AuthenticatedDevice>>,
) -> impl IntoResponse {
    let context_id: ContextId = match context_id.parse() {
        Ok(id) => id,
        Err(_) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid context ID format".to_owned(),
            }
            .into_response();
        }
    };

    // A byte count is small, but it is still a fact about a context: how much
    // state it holds, and — sampled over time — how busy it is. `GET
    // /admin-api/contexts/:id` already refuses a caller who is not in the
    // owning group, and answering here would hand back a measurement of the
    // very context that read withholds.
    //
    // Same predicate as that route, resolved per request. A node owner, and a
    // node running with no auth guard, get `NodeWide` and are unaffected.
    let scope = match list_scope_for(&state.ctx_client, node_owner, account, device) {
        Ok(scope) => scope,
        Err(err) => {
            error!(context_id=%context_id, error=?err, "Failed to resolve the caller's list scope");
            return parse_api_error(err).into_response();
        }
    };

    match admits_context(state.ctx_client.datastore(), &context_id, &scope) {
        Ok(true) => {}
        Ok(false) => {
            info!(context_id=%context_id, "Refusing context storage: caller is not a member of this context's group");
            return ApiError {
                status_code: StatusCode::FORBIDDEN,
                message: "account is not a member of the group owning this context".to_owned(),
            }
            .into_response();
        }
        Err(err) => {
            error!(context_id=%context_id, error=?err, "Failed to resolve the context's group");
            return parse_api_error(err).into_response();
        }
    }

    let size_in_bytes = context_storage_bytes(&state.store, context_id.as_ref());
    info!(context_id=%context_id, size_in_bytes, "Reporting context storage");

    ApiResponse {
        payload: GetContextStorageResponse::new(size_in_bytes),
    }
    .into_response()
}
