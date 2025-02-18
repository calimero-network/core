use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::alias::Alias;
use calimero_server_primitives::admin::DeleteAliasResponse;
use calimero_store::key::{Aliasable, StoreScopeCompat};
use reqwest::StatusCode;

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
        return (StatusCode::BAD_REQUEST, "invalid path params").into_response();
    };

    if let Err(err) = state.ctx_manager.delete_alias(alias, scope) {
        return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
    }

    ApiResponse {
        payload: DeleteAliasResponse::new(),
    }
    .into_response()
}
