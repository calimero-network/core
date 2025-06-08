use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::alias::Alias;
use calimero_server_primitives::admin::ListAliasesResponse;
use calimero_store::key::{Aliasable, StoreScopeCompat};
use reqwest::StatusCode;
use serde::Serialize;

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler<T>(
    Extension(state): Extension<Arc<AdminState>>,
    maybe_scope: Option<Path<T::Scope>>,
) -> impl IntoResponse
where
    T: Aliasable<Scope: StoreScopeCompat + Copy + PartialEq> + Serialize + From<[u8; 32]> + Eq,
{
    let scope = maybe_scope.map(|Path(s)| s);

    let aliases_raw = match state.node_client.list_aliases::<T>(scope) {
        Ok(a) => a,
        Err(err) => return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    };

    let data: HashMap<Alias<T>, T> = aliases_raw
        .into_iter()
        .map(|(alias, value, _scope)| (alias, value))
        .collect();

    ApiResponse {
        payload: ListAliasesResponse::new(data),
    }
    .into_response()
}
