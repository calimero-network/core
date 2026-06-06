use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::AbortMigrationRequest;
use calimero_server_primitives::admin::AbortMigrationApiResponse;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// `POST /admin-api/groups/{namespace_id}/migration/abort` — operator-facing
/// logical abort of an in-flight namespace migration (Task 6d.4).
///
/// Mirrors [`super::get_cascade_status::handler`]: parse the namespace id,
/// dispatch the abort to the context actor (which admin-gates it), map the
/// typed result into the admin JSON shape. Idempotent — aborting with nothing
/// pending returns `aborted: false`. An already-committed v2 context is not
/// recalled (documented scope; the abort only stops the rollout going forward).
pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(namespace_id=%namespace_id_str, "Aborting migration");

    let result = state
        .ctx_client
        .abort_migration(AbortMigrationRequest { namespace_id })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(response) => ApiResponse {
            payload: AbortMigrationApiResponse {
                namespace_id: hex::encode(response.namespace_id.to_bytes()),
                aborted: response.aborted,
            },
        }
        .into_response(),
        Err(err) => {
            error!(namespace_id=%namespace_id_str, error=?err, "Failed to abort migration");
            err.into_response()
        }
    }
}
