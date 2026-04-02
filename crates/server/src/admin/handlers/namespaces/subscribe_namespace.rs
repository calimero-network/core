use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use serde::Serialize;
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

#[derive(Debug, Serialize)]
pub struct SubscribeNamespaceResponse {
    pub subscribed: bool,
}

pub async fn handler(
    Path(namespace_id_hex): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let namespace_id: [u8; 32] = match hex::decode(&namespace_id_hex)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
    {
        Some(id) => id,
        None => {
            return parse_api_error(eyre::eyre!("invalid namespace_id hex")).into_response();
        }
    };

    info!(
        namespace_id = %namespace_id_hex,
        "Subscribing to namespace topic"
    );

    match state.node_client.subscribe_namespace(namespace_id).await {
        Ok(()) => ApiResponse {
            payload: SubscribeNamespaceResponse { subscribed: true },
        }
        .into_response(),
        Err(err) => {
            error!(?err, "Failed to subscribe to namespace");
            parse_api_error(err).into_response()
        }
    }
}
