use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::GetPeersCountResponse;

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn get_peers_count_handler(
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let peer_count = state.node_client.get_peers_count(None).await;

    ApiResponse {
        payload: GetPeersCountResponse::new(peer_count),
    }
    .into_response()
}
