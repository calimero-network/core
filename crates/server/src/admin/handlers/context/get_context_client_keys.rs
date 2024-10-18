use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::ClientKey;
use serde::{Deserialize, Serialize};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::admin::storage::client_keys::get_context_client_key;
use crate::AdminState;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientKeys {
    client_keys: Vec<ClientKey>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GetContextClientKeysResponse {
    data: ClientKeys,
}

pub async fn handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    // todo! experiment with Interior<Store>: WriteLayer<Interior>
    let client_keys_result = get_context_client_key(&state.store.clone(), &context_id)
        .map_err(|err| parse_api_error(err).into_response());
    match client_keys_result {
        Ok(client_keys) => ApiResponse {
            payload: GetContextClientKeysResponse {
                data: ClientKeys { client_keys },
            },
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
