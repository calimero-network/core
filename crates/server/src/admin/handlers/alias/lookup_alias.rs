use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::alias::Alias;
use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{AliasKind, LookupAliasResponse, LookupAliasResponseData};
use calimero_store::key::{Aliasable, StoreScopeCompat};
use reqwest::StatusCode;
use serde::Serialize;

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler<T: Aliasable<Scope: StoreScopeCompat>>(
    Extension(state): Extension<Arc<AdminState>>,
    scope: Option<Path<T::Scope>>,
    Path(alias): Path<Alias<T>>,
) -> impl IntoResponse
where
    T: AliasKind + From<Hash> + Serialize,
{
    let scope = scope.map(|Path(scope)| scope);

    match state.ctx_manager.lookup_alias(alias, scope) {
        Ok(value) => ApiResponse {
            payload: LookupAliasResponse {
                data: LookupAliasResponseData::new(value),
            },
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}
