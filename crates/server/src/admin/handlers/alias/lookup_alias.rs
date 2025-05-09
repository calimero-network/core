use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::alias::Alias;
use calimero_server_primitives::admin::{LookupAliasResponse, LookupAliasResponseData};
use calimero_store::key::{Aliasable, StoreScopeCompat};
use reqwest::StatusCode;
use serde::Serialize;

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler<T>(
    Extension(state): Extension<Arc<AdminState>>,
    plain_alias: Option<Path<Alias<T>>>,
    scoped_alias: Option<Path<(T::Scope, Alias<T>)>>,
) -> impl IntoResponse
where
    T: Aliasable<Scope: StoreScopeCompat> + Serialize + From<[u8; 32]>,
{
    let Some((alias, scope)) = plain_alias
        .map(|Path(alias)| (alias, None))
        .or_else(|| scoped_alias.map(|Path((scope, alias))| (alias, Some(scope))))
    else {
        return (StatusCode::BAD_REQUEST, "invalid path params").into_response();
    };

    match state.node_client.lookup_alias(alias, scope) {
        Ok(value) => ApiResponse {
            payload: LookupAliasResponse {
                data: LookupAliasResponseData::new(value),
            },
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}
