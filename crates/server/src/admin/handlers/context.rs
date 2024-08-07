use std::str::FromStr;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::identity::{KeyPair, PublicKey};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;

use crate::admin::service::{parse_api_error, AdminState, ApiError, ApiResponse, Empty};
use crate::admin::storage::client_keys::get_context_client_key;

#[derive(Debug, Serialize, Deserialize)]
pub struct ContextObject {
    context: calimero_primitives::context::Context,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetContextResponse {
    data: ContextObject,
}

pub async fn get_context_handler(
    Path(context_id): Path<calimero_primitives::context::ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    // todo! experiment with Interior<Store>: WriteLayer<Interior>
    let context = state
        .ctx_manager
        .get_context(&context_id)
        .map_err(|err| parse_api_error(err).into_response());

    match context {
        Ok(ctx) => match ctx {
            Some(context) => ApiResponse {
                payload: GetContextResponse {
                    data: ContextObject { context },
                },
            }
            .into_response(),
            None => ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Context not found".into(),
            }
            .into_response(),
        },
        Err(err) => err.into_response(),
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientKeys {
    client_keys: Vec<calimero_primitives::identity::ClientKey>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetContextClientKeysResponse {
    data: ClientKeys,
}

pub async fn get_context_client_keys_handler(
    Path(context_id): Path<calimero_primitives::context::ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    // todo! experiment with Interior<Store>: WriteLayer<Interior>
    let client_keys_result = get_context_client_key(&mut state.store.clone(), &context_id)
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContextUsers {
    context_users: Vec<calimero_primitives::identity::ContextUser>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetContextUsersResponse {
    data: ContextUsers,
}

pub async fn get_context_users_handler(
    Path(_context_id): Path<String>,
    Extension(_state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    ApiResponse {
        payload: GetContextUsersResponse {
            data: ContextUsers {
                context_users: vec![],
            },
        },
    }
    .into_response()
}

pub async fn get_contexts_handler(
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    // todo! experiment with Interior<Store>: WriteLayer<Interior>
    let contexts = state
        .ctx_manager
        .get_contexts(None)
        .map_err(parse_api_error);

    match contexts {
        Ok(contexts) => ApiResponse {
            payload: calimero_server_primitives::admin::GetContextsResponse {
                data: calimero_server_primitives::admin::ContextList { contexts },
            },
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedContext {
    is_deleted: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteContextResponse {
    data: DeletedContext,
}

pub async fn delete_context_handler(
    Path(context_id): Path<String>,
    _session: Session,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let context_id_result = match calimero_primitives::context::ContextId::from_str(&context_id) {
        Ok(context_id) => context_id,
        Err(_) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid context id".into(),
            }
            .into_response();
        }
    };

    // todo! experiment with Interior<Store>: WriteLayer<Interior>
    let result = state
        .ctx_manager
        .delete_context(&context_id_result)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(result) => ApiResponse {
            payload: DeleteContextResponse {
                data: DeletedContext { is_deleted: result },
            },
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}

pub async fn create_context_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<calimero_server_primitives::admin::CreateContextRequest>,
) -> impl IntoResponse {
    // Create a Send-able RNG
    let mut rng = StdRng::from_entropy();

    // Generate a key pair for the context ID
    let mut context_seed = [0u8; 32];
    rng.fill_bytes(&mut context_seed);
    let context_signing_key = SigningKey::from_bytes(&context_seed);
    let context_verifying_key = VerifyingKey::from(&context_signing_key);
    let context_id =
        calimero_primitives::context::ContextId::from(*context_verifying_key.as_bytes());

    // Generate a separate key pair for the member's identity
    let mut member_seed = [0u8; 32];
    rng.fill_bytes(&mut member_seed);
    let member_signing_key = SigningKey::from_bytes(&member_seed);
    let member_verifying_key = VerifyingKey::from(&member_signing_key);

    let context = calimero_primitives::context::Context {
        id: context_id,
        application_id: req.application_id,
        last_transaction_hash: Default::default(),
    };

    let initial_identity = KeyPair {
        public_key: PublicKey(*member_verifying_key.as_bytes()),
        private_key: Some(*member_signing_key.as_bytes()),
    };

    // todo! experiment with Interior<Store>: WriteLayer<Interior>
    let result = state
        .ctx_manager
        .add_context(context.clone(), initial_identity)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(_) => ApiResponse {
            payload: calimero_server_primitives::admin::CreateContextResponse {
                data: calimero_server_primitives::admin::ContextResponse {
                    context,
                    member_public_key: (*member_verifying_key.as_bytes()).into(),
                },
            },
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}

#[derive(Debug, Serialize)]
struct GetContextStorageResponse {
    data: calimero_server_primitives::admin::ContextStorage,
}

pub async fn get_context_storage_handler(
    Path(_context_id): Path<String>,
    Extension(_state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    ApiResponse {
        payload: GetContextStorageResponse {
            data: calimero_server_primitives::admin::ContextStorage { size_in_bytes: 0 },
        },
    }
    .into_response()
}

#[derive(Deserialize)]
pub struct JoinContextRequest {
    pub public_key: PublicKey,
    pub private_key: [u8; 32],
}

#[derive(Debug, Serialize)]
struct JoinContextResponse {
    data: Empty,
}

pub async fn join_context_handler(
    Path(context_id): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(request): Json<JoinContextRequest>,
) -> impl IntoResponse {
    let context_id_result = match calimero_primitives::context::ContextId::from_str(&context_id) {
        Ok(context_id) => context_id,
        Err(_) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid context id".into(),
            }
            .into_response();
        }
    };

    // Create a KeyPair from the provided public and private keys
    let initial_identity = KeyPair {
        public_key: request.public_key,
        private_key: Some(request.private_key),
    };

    let result = state
        .ctx_manager
        .join_context(&context_id_result, initial_identity)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(_) => ApiResponse {
            payload: JoinContextResponse { data: Empty {} },
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}

#[derive(Debug, Serialize)]
struct UpdateApplicationIdResponse {
    data: Empty,
}

pub async fn update_application_id(
    Extension(state): Extension<Arc<AdminState>>,
    Path(context_id): Path<String>,
    Json(req): Json<calimero_server_primitives::admin::UpdateContextApplicationRequest>,
) -> impl IntoResponse {
    let context_id_result = match calimero_primitives::context::ContextId::from_str(&context_id) {
        Ok(context_id) => context_id,
        Err(_) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid context id".into(),
            }
            .into_response();
        }
    };

    let result = state
        .ctx_manager
        .update_application_id(context_id_result, req.application_id)
        .map_err(parse_api_error);

    match result {
        Ok(_) => ApiResponse {
            payload: UpdateApplicationIdResponse { data: Empty {} },
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
