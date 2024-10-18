use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::identity::ContextUser;
use serde::{Deserialize, Serialize};

use crate::admin::service::ApiResponse;
use crate::AdminState;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextUsers {
    context_users: Vec<ContextUser>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GetContextUsersResponse {
    data: ContextUsers,
}

pub async fn handler(
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
