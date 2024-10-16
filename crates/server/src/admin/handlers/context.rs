use core::str::FromStr;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::identity::{ClientKey, ContextUser, PublicKey};
use calimero_server_primitives::admin::{
    ContextStorage, CreateContextRequest, CreateContextResponse, GetContextsResponse,
    JoinContextRequest, JoinContextResponse, JoinContextResponseData,
    UpdateContextApplicationRequest,
};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use tracing::error;

use crate::admin::service::{parse_api_error, ApiError, ApiResponse, Empty};
use crate::admin::storage::client_keys::get_context_client_key;
use crate::AdminState;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextObject {
    context: Context,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GetContextResponse {
    data: ContextObject,
}

pub async fn get_context_handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    // todo! experiment with Interior<Store>: WriteLayer<Interior>
    let context = state
        .ctx_manager
        .get_context(&context_id)
        .map_err(|err| parse_api_error(err).into_response());

    #[expect(clippy::option_if_let_else, reason = "Clearer here")]
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
pub struct GetContextIdentitiesResponse {
    data: ContextIdentities,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextIdentities {
    identities: Vec<PublicKey>,
}

pub async fn get_context_identities_handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let context = state
        .ctx_manager
        .get_context(&context_id)
        .map_err(|err| parse_api_error(err).into_response());

    match context {
        #[expect(clippy::option_if_let_else, reason = "Clearer here")]
        Ok(ctx) => match ctx {
            Some(context) => {
                let context_identities = state
                    .ctx_manager
                    .get_context_owned_identities(context.id)
                    .map_err(|err| parse_api_error(err).into_response());

                match context_identities {
                    Ok(identities) => ApiResponse {
                        payload: GetContextIdentitiesResponse {
                            data: ContextIdentities { identities },
                        },
                    }
                    .into_response(),
                    Err(err) => {
                        error!("Error getting context identities: {:?}", err);
                        ApiError {
                            status_code: StatusCode::INTERNAL_SERVER_ERROR,
                            message: "Something went wrong".into(),
                        }
                        .into_response()
                    }
                }
            }
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
    client_keys: Vec<ClientKey>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GetContextClientKeysResponse {
    data: ClientKeys,
}

pub async fn get_context_client_keys_handler(
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextUsers {
    context_users: Vec<ContextUser>,
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
            payload: GetContextsResponse::new(contexts),
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
    let Ok(context_id_result) = ContextId::from_str(&context_id) else {
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
    Json(req): Json<CreateContextRequest>,
) -> impl IntoResponse {
    let result = state
        .ctx_manager
        .create_context(
            req.context_seed.map(Into::into),
            req.application_id,
            None,
            req.initialization_params,
        )
        .await
        .map_err(parse_api_error);

    match result {
        Ok((context_id, member_public_key)) => ApiResponse {
            payload: CreateContextResponse::new(context_id, member_public_key),
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetContextStorageResponse {
    data: ContextStorage,
}

impl GetContextStorageResponse {
    #[must_use]
    pub const fn new(size_in_bytes: u64) -> Self {
        Self {
            data: ContextStorage::new(size_in_bytes),
        }
    }
}

pub async fn get_context_storage_handler(
    Path(_context_id): Path<String>,
    Extension(_state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    ApiResponse {
        payload: GetContextStorageResponse::new(0),
    }
    .into_response()
}

pub async fn join_context_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(JoinContextRequest {
        private_key,
        invitation_payload,
        ..
    }): Json<JoinContextRequest>,
) -> impl IntoResponse {
    let result = state
        .ctx_manager
        .join_context(private_key, invitation_payload)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(r) => ApiResponse {
            payload: JoinContextResponse::new(r.map(|(context_id, member_public_key)| {
                JoinContextResponseData::new(context_id, member_public_key)
            })),
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
    Json(req): Json<UpdateContextApplicationRequest>,
) -> impl IntoResponse {
    let Ok(context_id_result) = ContextId::from_str(&context_id) else {
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
