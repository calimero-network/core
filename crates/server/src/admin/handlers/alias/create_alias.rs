use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{AliasKind, CreateAliasRequest, CreateAliasResponse};
use calimero_store::key::{Aliasable, StoreScopeCompat};
use reqwest::StatusCode;

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler<T>(
    Extension(state): Extension<Arc<AdminState>>,
    scope: Option<Path<T::Scope>>,
    Json(CreateAliasRequest { alias, value }): Json<CreateAliasRequest<T>>,
) -> impl IntoResponse
where
    T: Aliasable<Scope: StoreScopeCompat> + AliasKind + Into<Hash>,
{
    let scope = scope.map(|Path(scope)| scope);

    if let Err(err) = state
        .ctx_manager
        .create_alias(alias, scope, T::from_value(value))
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
    }

    ApiResponse {
        payload: CreateAliasResponse::new(),
    }
    .into_response()
}
