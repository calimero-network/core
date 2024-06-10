use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::identity::{ClientKey, Context};
use rand::RngCore;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;

use crate::admin::service::{parse_api_error, AdminState, ApiError, ApiResponse};
use crate::admin::storage::context::{add_context, delete_context, get_context, get_contexts};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextData {
    context: Context,
    client_keys: Vec<ClientKey>,
    users: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetContextResponse {
    data: ContextData,
}

pub async fn get_context_handler(
    Path(context_id): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let context_result =
        get_context(&state.store, &context_id).map_err(|err| parse_api_error(err).into_response());

    match context_result {
        Ok(ctx) => match ctx {
            Some(context) => ApiResponse {
                payload: GetContextResponse { data: ContextData { context, client_keys: vec![], users: vec![] } },
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
pub struct GetContextsResponse {
    data: Vec<Context>,
}

pub async fn get_contexts_handler(
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let contexts = get_contexts(&state.store).map_err(|err| parse_api_error(err));
    return match contexts {
        Ok(contexts) => ApiResponse {
            payload: GetContextsResponse { data: contexts },
        }
        .into_response(),
        Err(err) => err.into_response(),
    };
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteContextResponse {
    data: bool,
}

pub async fn delete_context_handler(
    Path(context_id): Path<String>,
    _session: Session,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let result = delete_context(&state.store, &context_id).map_err(|err| parse_api_error(err));
    return match result {
        Ok(result) => ApiResponse {
            payload: DeleteContextResponse { data: result },
        }
        .into_response(),
        Err(err) => err.into_response(),
    };
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContextRequest {
    application_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateContextResponse {
    data: Context,
}

pub async fn create_context_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<CreateContextRequest>,
) -> impl IntoResponse {
    let mut seed = [0; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&mut seed);
    let context_id = signing_key.verifying_key();

    let context = Context {
        id: bs58::encode(&context_id).into_string(),
        signing_key,
        application_id: req.application_id,
    };

    let result = add_context(&state.store, context.clone()).map_err(|err| parse_api_error(err));

    let response = match result {
        Ok(_) => ApiResponse {
            payload: CreateContextResponse { data: context },
        }
        .into_response(),
        Err(err) => err.into_response(),
    };

    response
}
