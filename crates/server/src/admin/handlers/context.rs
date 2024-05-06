use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::identity::Context;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use uuid::Uuid;

use super::add_client_key::parse_api_error;
use crate::admin::service::{AdminState, ApiError, ApiResponse};
use crate::admin::storage::context::{add_context, delete_context, get_context, get_contexts};

#[derive(Debug, Serialize, Deserialize)]
pub struct GetContextResponse {
    data: Context,
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
                payload: GetContextResponse { data: context },
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
    let context = Context {
        id: Uuid::new_v4().to_string(),
        identity: libp2p::identity::Keypair::generate_ed25519(),
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
