use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

#[derive(Debug, Deserialize, Serialize)]
pub struct GetContextIdentitiesResponse {
    data: ContextIdentities,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextIdentities {
    identities: Vec<PublicKey>,
}

pub async fn handler(
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
