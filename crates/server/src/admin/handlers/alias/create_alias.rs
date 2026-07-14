use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_node_primitives::client::AliasExists;
use calimero_server_primitives::admin::{AliasKind, CreateAliasRequest, CreateAliasResponse};
use calimero_store::key::{Aliasable, StoreScopeCompat};
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
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

    // `create_alias` does the existence check + write on one store handle and
    // returns a typed `AliasExists` on conflict, which we map to 409 — no racy
    // caller-side pre-check. (Alias length/charset are already enforced by
    // `Alias`'s deserializer, so a malformed alias 400s before reaching here.)
    if let Err(err) = state
        .node_client
        .create_alias(alias, scope, T::from_value(value))
    {
        if err.downcast_ref::<AliasExists>().is_some() {
            return ApiError {
                status_code: StatusCode::CONFLICT,
                message: "alias already exists".to_owned(),
            }
            .into_response();
        }
        error!(alias=%alias, error=?err, "Failed to create alias");
        return parse_api_error(err).into_response();
    }

    info!(alias=%alias, "Alias created successfully");

    ApiResponse {
        payload: CreateAliasResponse::new(),
    }
    .into_response()
}
