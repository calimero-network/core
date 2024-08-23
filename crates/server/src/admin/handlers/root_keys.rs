use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::identity::{RootKey, WalletType};
use calimero_server_primitives::admin::{AddPublicKeyRequest, IntermediateAddPublicKeyRequest};
use calimero_store::Store;
use chrono::Utc;
use futures_util::TryFutureExt;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::add_client_key::transform_request;
use crate::admin::service::{parse_api_error, AdminState, ApiError, ApiResponse, Empty};
use crate::admin::storage::root_key::{add_root_key, clean_auth_keys};
use crate::admin::utils::auth::validate_challenge;

#[derive(Debug, Serialize)]
struct CreateRootKeyResponse {
    data: String,
}

pub async fn create_root_key_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(intermediate_req): Json<IntermediateAddPublicKeyRequest>,
) -> impl IntoResponse {
    async {transform_request(intermediate_req)}
        .and_then(|req| validate_challenge(req, &state.keypair))
        .and_then(|req| async {store_root(req, &mut state.store.clone())})
        .await
        .map_or_else(
            |err| err.into_response(),
            |_| {
                let data: String = "Root key stored".to_string();
                ApiResponse {
                    payload: CreateRootKeyResponse { data },
                }
                .into_response()
            },
        )
}

pub fn store_root(
    req: AddPublicKeyRequest,
    store: &mut Store,
) -> Result<AddPublicKeyRequest, ApiError> {
    store_root_key(
        req.wallet_metadata.signing_key.clone(),
        req.wallet_metadata.wallet_type.clone(),
        store,
    )?;
    Ok(req)
}

pub fn store_root_key(
    signing_key: String,
    wallet_type: WalletType,
    store: &mut Store,
) -> Result<bool, ApiError> {
    let root_key = RootKey {
        signing_key,
        wallet_type,
        created_at: Utc::now().timestamp_millis() as u64,
    };
    add_root_key(store, root_key).map_err(parse_api_error)?;

    info!("Root key stored successfully.");
    Ok(true)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteKeysResponse {
    data: Empty,
}
pub async fn delete_auth_keys_handler(
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    clean_auth_keys(&mut state.store.clone()).map_or_else(
        |err| parse_api_error(err).into_response(),
        |_| {
            ApiResponse {
                payload: DeleteKeysResponse { data: Empty {} },
            }
            .into_response()
        },
    );
}
