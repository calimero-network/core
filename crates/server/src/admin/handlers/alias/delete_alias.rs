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

pub async fn handler<T: Aliasable<Scope: StoreScopeCompat>>(
    Extension(state): Extension<Arc<AdminState>>,
    scope: Option<Path<T::Scope>>,
    Path(alias): Path<Alias<T>>,
) -> impl IntoResponse {
    let scope = scope.map(|Path(scope)| scope);

    if let Err(err) = state.ctx_manager.delete_alias(alias, scope) {
        return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
    }

    ApiResponse {
        payload: DeleteAliasResponse::new(),
    }
    .into_response()
}
