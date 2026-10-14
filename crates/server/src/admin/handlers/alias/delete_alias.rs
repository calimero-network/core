use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::alias::Alias;
use calimero_server_primitives::admin::DeleteAliasResponse;
use calimero_store::key::{Aliasable, StoreScopeCompat};
use tracing::{error, info};

use super::{outside_binding, BoundKeyTarget};
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::ClientKeyScope;
use crate::AdminState;

pub async fn handler<T>(
    Extension(state): Extension<Arc<AdminState>>,
    client_key: Option<Extension<ClientKeyScope>>,
    Path(alias): Path<Alias<T>>,
) -> impl IntoResponse
where
    T: Aliasable<Scope: StoreScopeCompat> + From<[u8; 32]> + BoundKeyTarget,
{
    info!(alias=%alias, "Deleting alias");

    if let Some(Extension(key)) = client_key {
        match state.node_client.lookup_alias(alias, None) {
            Ok(Some(value)) if !value.permitted(&key, state.ctx_client.datastore()) => {
                return outside_binding();
            }
            Ok(_) => {}
            Err(err) => {
                error!(alias=%alias, error=?err, "Failed to look up alias before delete");
                return parse_api_error(err).into_response();
            }
        }
    }

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
