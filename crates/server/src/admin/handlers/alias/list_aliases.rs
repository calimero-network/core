use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{AliasRecord, ListAliasesResponse};
use calimero_store::key::{Aliasable, StoreScopeCompat};
use serde::{Deserialize, Serialize};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler<T>(
    Extension(state): Extension<Arc<AdminState>>,
    maybe_scope: Option<Path<T::Scope>>,
) -> impl IntoResponse
where
    T: Aliasable + Serialize + Clone + From<[u8; 32]>,
    T::Scope: Copy + PartialEq + StoreScopeCompat,
{
    let scope = maybe_scope.map(|Path(s)| s);

    let aliases_raw = match state.node_client.list_aliases::<T>(scope) {
        Ok(a) => a,
        Err(err) => return parse_api_error(err).into_response(),
    };

    let aliases: Vec<AliasRecord<T>> = aliases_raw
        .into_iter()
        .map(|(alias, value, _scope)| AliasRecord { alias, value })
        .collect();

    ApiResponse {
        payload: ListAliasesResponse::new(aliases),
    }
    .into_response()
}
