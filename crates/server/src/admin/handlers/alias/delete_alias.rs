use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::alias::Alias;
use calimero_server_primitives::admin::DeleteAliasResponse;
use calimero_store::key::{Aliasable, StoreScopeCompat};
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler<T>(
    Extension(state): Extension<Arc<AdminState>>,
    Path(alias): Path<Alias<T>>,
) -> impl IntoResponse
where
    T: Aliasable<Scope: StoreScopeCompat>,
{
    info!(alias=%alias, "Deleting alias");

    // Delete is idempotent: removing an absent alias succeeds (the store delete
    // doesn't distinguish), matching the SDK's expectation.
    if let Err(err) = state.node_client.delete_alias(alias, None) {
        error!(alias=%alias, error=?err, "Failed to delete alias");
        return parse_api_error(err).into_response();
    }

    info!(alias=%alias, "Alias deleted successfully");

    ApiResponse {
        payload: DeleteAliasResponse::new(),
    }
    .into_response()
}
