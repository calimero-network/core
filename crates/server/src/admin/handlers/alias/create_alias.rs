use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::admin::{AliasKind, CreateAliasRequest, CreateAliasResponse};
use calimero_store::key::{Aliasable, StoreScopeCompat};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler<T>(
    Extension(state): Extension<Arc<AdminState>>,
    scope: Option<Path<T::Scope>>,
    Json(CreateAliasRequest { alias, value }): Json<CreateAliasRequest<T>>,
) -> impl IntoResponse
where
    T: Aliasable<Scope: StoreScopeCompat> + AliasKind + AsRef<[u8; 32]>,
{
    let scope = scope.map(|Path(scope)| scope);

    info!(alias=%alias, "Creating alias");

    if let Err(err) = state
        .node_client
        .create_alias(alias, scope, T::from_value(value))
    {
        error!(alias=%alias, error=?err, "Failed to create alias");
        return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
    }

    info!(alias=%alias, "Alias created successfully");

    ApiResponse {
        payload: CreateAliasResponse::new(),
    }
    .into_response()
}
