use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::alias::Alias;
use calimero_server_primitives::admin::DeleteAliasResponse;
use calimero_store::key::{Aliasable, StoreScopeCompat};
use tracing::{error, info};

use super::BoundKeyTarget;
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

    // Outside a bound key's binding the alias reads as unset, as in lookup, so its
    // delete answers as deleting an absent one and leaves it in place.
    let store = state.ctx_client.datastore();
    let deleted = state.node_client.delete_alias_unless(alias, |value| {
        client_key
            .as_ref()
            .is_some_and(|key| !value.permitted(key, store))
    });
    if let Err(err) = deleted {
        error!(alias=%alias, error=?err, "Failed to delete alias");
        return parse_api_error(err).into_response();
    }

    info!(alias=%alias, "Alias deleted successfully");

    ApiResponse {
        payload: DeleteAliasResponse::new(),
    }
    .into_response()
}
