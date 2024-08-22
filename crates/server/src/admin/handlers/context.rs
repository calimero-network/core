use std::str::FromStr;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;

use crate::admin::service::{parse_api_error, AdminState, ApiError, ApiResponse, Empty};
use crate::admin::storage::client_keys::get_context_client_key;
use crate::admin::utils::context::{create_context, join_context};

#[derive(Debug, Deserialize, Serialize)]
pub struct ContextObject {
    context: calimero_primitives::context::Context,
}

#[derive(Debug, Deserialize, Serialize)]
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

    #[allow(clippy::option_if_let_else)]
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientKeys {
    client_keys: Vec<calimero_primitives::identity::ClientKey>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GetContextClientKeysResponse {
    data: ClientKeys,
}

pub async fn get_context_client_keys_handler(
    Path(context_id): Path<calimero_primitives::context::ContextId>,
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextUsers {
    context_users: Vec<calimero_primitives::identity::ContextUser>,
}

#[derive(Debug, Deserialize, Serialize)]
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

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedContext {
    is_deleted: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct DeleteContextResponse {
    data: DeletedContext,
}

pub async fn delete_context_handler(
    Path(context_id): Path<String>,
    _session: Session,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let Ok(context_id_result) = calimero_primitives::context::ContextId::from_str(&context_id)
    else {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Invalid context id".into(),
        }
        .into_response();
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
    //TODO enable providing private key in the request
    let result = create_context(&state.ctx_manager, req.application_id, None)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(context_create_result) => ApiResponse {
            payload: calimero_server_primitives::admin::CreateContextResponse {
                data: calimero_server_primitives::admin::ContextResponse {
                    context: context_create_result.context,
                    member_public_key: context_create_result.identity.public_key,
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

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct JoinContextRequest {
    pub private_key: [u8; 32],
}

#[derive(Debug, Serialize)]
struct JoinContextResponse {
    data: Empty,
}

pub async fn join_context_handler(
    Path(context_id): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    request: Option<Json<JoinContextRequest>>,
) -> impl IntoResponse {
    let Ok(context_id_result) = calimero_primitives::context::ContextId::from_str(&context_id)
    else {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Invalid context id".into(),
        }
        .into_response();
    };

    let private_key = if let Some(Json(json_body)) = request {
        Some(bs58::encode(json_body.private_key).into_string())
    } else {
        None
    };

    let result = join_context(
        &state.ctx_manager,
        context_id_result,
        private_key.as_deref(),
    )
    .await
    .map_err(parse_api_error);

    match result {
        Ok(()) => ApiResponse {
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
    let Ok(context_id_result) = calimero_primitives::context::ContextId::from_str(&context_id)
    else {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Invalid context id".into(),
        }
        .into_response();
    };

    let result = state
        .ctx_manager
        .update_application_id(context_id_result, req.application_id)
        .map_err(parse_api_error);

    match result {
        Ok(()) => ApiResponse {
            payload: UpdateApplicationIdResponse { data: Empty {} },
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}
