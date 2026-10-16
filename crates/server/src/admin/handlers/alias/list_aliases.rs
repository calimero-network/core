use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::ListAliasesResponse;
use calimero_store::key::{Aliasable, StoreScopeCompat};
use serde::Serialize;
use tracing::{error, info};

use super::BoundKeyTarget;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::ClientKeyScope;
use crate::AdminState;

pub async fn handler<T>(
    Extension(state): Extension<Arc<AdminState>>,
    client_key: Option<Extension<ClientKeyScope>>,
    maybe_scope: Option<Path<T::Scope>>,
) -> impl IntoResponse
where
    T: Aliasable + Serialize + From<[u8; 32]> + BoundKeyTarget,
    T::Scope: Copy + PartialEq + StoreScopeCompat,
{
    let scope = maybe_scope.map(|Path(s)| s);

    info!("Listing aliases");

    let aliases = match state.node_client.list_aliases::<T>(scope) {
        Ok(aliases) => aliases,
        Err(err) => {
            error!(error=?err, "Failed to list aliases");
            return parse_api_error(err).into_response();
        }
    };

    let aliases = aliases
        .into_iter()
        .map(|(alias, value, _scope)| (alias, value))
        .filter(|(_, value)| {
            client_key
                .as_ref()
                .is_none_or(|key| value.permitted(key, state.ctx_client.datastore()))
        })
        .collect();

    info!("Aliases listed successfully");

    ApiResponse {
        payload: ListAliasesResponse::new(aliases),
    }
    .into_response()
}
