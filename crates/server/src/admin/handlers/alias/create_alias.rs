use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
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
    T: Aliasable<Scope: StoreScopeCompat + Copy> + AliasKind + AsRef<[u8; 32]>,
{
    let scope = scope.map(|Path(scope)| scope);

    info!(alias=%alias, "Creating alias");

    // Reject a duplicate with 409 instead of letting the store's generic
    // "alias already exists" error fall through to a 500. (Alias length/charset
    // are already enforced by `Alias`'s deserializer, so a malformed alias 400s
    // before reaching here.)
    match state.node_client.alias_exists(alias, scope) {
        Ok(true) => {
            return ApiError {
                status_code: StatusCode::CONFLICT,
                message: "alias already exists".to_owned(),
            }
            .into_response();
        }
        Ok(false) => {}
        Err(err) => {
            error!(alias=%alias, error=?err, "Failed to check alias existence");
            return parse_api_error(err).into_response();
        }
    }

    if let Err(err) = state
        .node_client
        .create_alias(alias, scope, T::from_value(value))
    {
        error!(alias=%alias, error=?err, "Failed to create alias");
        return parse_api_error(err).into_response();
    }

    info!(alias=%alias, "Alias created successfully");

    ApiResponse {
        payload: CreateAliasResponse::new(),
    }
    .into_response()
}
