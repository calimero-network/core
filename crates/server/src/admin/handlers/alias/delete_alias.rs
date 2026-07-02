use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::alias::Alias;
use calimero_server_primitives::admin::DeleteAliasResponse;
use calimero_store::key::{Aliasable, StoreScopeCompat};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler<T>(
    Extension(state): Extension<Arc<AdminState>>,
    plain_alias: Option<Path<Alias<T>>>,
    scoped_alias: Option<Path<(T::Scope, Alias<T>)>>,
) -> impl IntoResponse
where
    T: Aliasable<Scope: StoreScopeCompat + Copy>,
{
    let Some((alias, scope)) = plain_alias
        .map(|Path(alias)| (alias, None))
        .or_else(|| scoped_alias.map(|Path((scope, alias))| (alias, Some(scope))))
    else {
        error!("Invalid path params for delete alias");
        return (StatusCode::BAD_REQUEST, "invalid path params").into_response();
    };

    info!(alias=%alias, "Deleting alias");

    // Deleting an alias that doesn't exist is a 404 rather than a silent 200
    // (the underlying store delete is idempotent and wouldn't surface it).
    match state.node_client.alias_exists(alias, scope) {
        Ok(true) => {}
        Ok(false) => {
            info!(alias=%alias, "Alias not found");
            return (StatusCode::NOT_FOUND, "alias not found").into_response();
        }
        Err(err) => {
            error!(alias=%alias, error=?err, "Failed to check alias existence");
            return parse_api_error(err).into_response();
        }
    }

    if let Err(err) = state.node_client.delete_alias(alias, scope) {
        error!(alias=%alias, error=?err, "Failed to delete alias");
        return parse_api_error(err).into_response();
    }

    info!(alias=%alias, "Alias deleted successfully");

    ApiResponse {
        payload: DeleteAliasResponse::new(),
    }
    .into_response()
}
