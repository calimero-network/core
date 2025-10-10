use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::alias::Alias;
use calimero_server_primitives::admin::DeleteAliasResponse;
use calimero_store::key::{Aliasable, StoreScopeCompat};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler<T>(
    Extension(state): Extension<Arc<AdminState>>,
    plain_alias: Option<Path<Alias<T>>>,
    scoped_alias: Option<Path<(T::Scope, Alias<T>)>>,
) -> impl IntoResponse
where
    T: Aliasable<Scope: StoreScopeCompat>,
{
    let Some((alias, scope)) = plain_alias
        .map(|Path(alias)| (alias, None))
        .or_else(|| scoped_alias.map(|Path((scope, alias))| (alias, Some(scope))))
    else {
        error!("Invalid path params for delete alias");
        return (StatusCode::BAD_REQUEST, "invalid path params").into_response();
    };

    info!(alias=%alias, "Deleting alias");

    if let Err(err) = state.node_client.delete_alias(alias, scope) {
        error!(alias=%alias, error=?err, "Failed to delete alias");
        return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
    }

    info!(alias=%alias, "Alias deleted successfully");

    ApiResponse {
        payload: DeleteAliasResponse::new(),
    }
    .into_response()
}
