use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::ListAliasesResponse;
use calimero_store::key::{Aliasable, StoreScopeCompat};
use serde::Serialize;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler<T>(
    Extension(state): Extension<Arc<AdminState>>,
    maybe_scope: Option<Path<T::Scope>>,
) -> impl IntoResponse
where
    T: Aliasable + Serialize + From<[u8; 32]>,
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
        .collect();

    info!("Aliases listed successfully");

    ApiResponse {
        payload: ListAliasesResponse::new(aliases),
    }
    .into_response()
}
