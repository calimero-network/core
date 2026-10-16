use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::alias::Alias;
use calimero_server_primitives::admin::{LookupAliasResponse, LookupAliasResponseData};
use calimero_store::key::{Aliasable, StoreScopeCompat};
use serde::Serialize;
use tracing::{error, info};

use super::BoundKeyTarget;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::ClientKeyScope;
use crate::AdminState;

pub async fn handler<T>(
    Extension(state): Extension<Arc<AdminState>>,
    client_key: Option<Extension<ClientKeyScope>>,
    Path(alias): Path<Alias<T>>,
) -> impl IntoResponse
where
    T: Aliasable<Scope: StoreScopeCompat> + Serialize + From<[u8; 32]> + BoundKeyTarget,
{
    info!(alias=%alias, "Looking up alias");

    // Lookup is a nullable getter: a missing alias returns 200 with a null
    // `value` (the SDK contract), not 404 — callers use it to check existence.
    match state.node_client.lookup_alias(alias, None) {
        Ok(value) => {
            // Outside a bound key's binding an alias reads as unset. Names are node-wide,
            // so creating a taken one still answers 409; its value is never shown.
            let value = value.filter(|v| {
                client_key
                    .as_ref()
                    .is_none_or(|key| v.permitted(key, state.ctx_client.datastore()))
            });
            info!(alias=%alias, found=%value.is_some(), "Alias lookup complete");
            ApiResponse {
                payload: LookupAliasResponse {
                    data: LookupAliasResponseData::new(value),
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(alias=%alias, error=?err, "Failed to lookup alias");
            parse_api_error(err).into_response()
        }
    }
}
