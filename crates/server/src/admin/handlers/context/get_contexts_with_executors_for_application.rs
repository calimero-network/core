use std::pin::pin;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PublicKey;
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

#[derive(Debug, Serialize, Deserialize)]
pub struct ContextWithExecutors {
    pub id: String,
    pub application_id: String,
    pub root_hash: String,
    pub executors: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetContextsWithExecutorsResponse {
    pub contexts: Vec<ContextWithExecutors>,
}

impl GetContextsWithExecutorsResponse {
    pub fn new(contexts: Vec<ContextWithExecutors>) -> Self {
        Self { contexts }
    }
}

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Path(application_id): Path<ApplicationId>,
) -> impl IntoResponse {
    info!(application_id=%application_id, "Getting contexts with executors for application");

    let context_ids = state.ctx_client.get_context_ids(None);
    let mut context_ids = pin!(context_ids);
    let mut contexts_with_executors = Vec::new();

    while let Some(context_id) = context_ids.try_next().await.transpose() {
        let context_id = match context_id {
            Ok(id) => id,
            Err(err) => {
                error!(application_id=%application_id, error=?err, "Failed to get context IDs");
                return parse_api_error(err).into_response();
            }
        };

        match state.ctx_client.get_context(&context_id) {
            Ok(Some(context)) => {
                // Filter contexts by application_id
                if context.application_id == application_id {
                    // Get executors (identities) for this context
                    let stream = state.ctx_client.get_context_members(&context.id, None);
                    let executors: Result<Vec<PublicKey>, _> =
                        stream.map_ok(|(id, _)| id).try_collect().await;

                    match executors {
                        Ok(executor_identities) => {
                            let context_with_executors = ContextWithExecutors {
                                id: context.id.to_string(),
                                application_id: context.application_id.to_string(),
                                root_hash: context.root_hash.to_string(),
                                executors: executor_identities
                                    .into_iter()
                                    .map(|pk| pk.to_string())
                                    .collect(),
                            };
                            contexts_with_executors.push(context_with_executors);
                        }
                        Err(err) => {
                            error!(application_id=%application_id, context_id=%context.id, error=?err, "Failed to get context executors");
                            // Still add the context but without executors
                            let context_with_executors = ContextWithExecutors {
                                id: context.id.to_string(),
                                application_id: context.application_id.to_string(),
                                root_hash: context.root_hash.to_string(),
                                executors: vec![],
                            };
                            contexts_with_executors.push(context_with_executors);
                        }
                    }
                }
            }
            Ok(None) => {
                // Context doesn't exist, skip
                continue;
            }
            Err(err) => {
                error!(application_id=%application_id, context_id=%context_id, error=?err, "Failed to get context");
                continue;
            }
        }
    }

    info!(application_id=%application_id, contexts_count=%contexts_with_executors.len(), "Retrieved contexts with executors for application");
    ApiResponse {
        payload: GetContextsWithExecutorsResponse::new(contexts_with_executors),
    }
    .into_response()
}
