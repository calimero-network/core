use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::LeaveContextRequest;
use calimero_server_primitives::admin::{LeaveContextApiResponse, LeaveContextApiResponseData};
use tracing::{error, info};

use crate::admin::handlers::groups::parse_context_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(context_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
) -> impl IntoResponse {
    let context_id = match parse_context_id(&context_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    // Authentication is enforced by the admin-API auth middleware; an
    // unauthenticated request never reaches this handler with `auth_key`
    // populated. We extract it here so authenticated callers' identity
    // appears in audit logs alongside the action — same shape as
    // `delete_group` / `delete_namespace`. The handler resolves the
    // actual member identity from local storage independently, so the
    // auth_key isn't a control point for *which* identity to leave —
    // it's the auth boundary for whether a leave is permitted at all.
    let auth_caller = auth_key.map(|Extension(k)| k.0);

    info!(
        context_id=%context_id_str,
        ?auth_caller,
        "Leaving context locally (no DAG op published)"
    );

    let result = state
        .ctx_client
        .leave_context(LeaveContextRequest { context_id })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(
                context_id=%resp.context_id,
                member=%resp.member_public_key,
                "Successfully left context locally"
            );
            ApiResponse {
                payload: LeaveContextApiResponse {
                    data: LeaveContextApiResponseData {
                        context_id: resp.context_id,
                        member_public_key: resp.member_public_key,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(context_id=%context_id_str, error=?err, "Failed to leave context");
            err.into_response()
        }
    }
}
