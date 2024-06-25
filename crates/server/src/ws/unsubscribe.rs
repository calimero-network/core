use std::sync::Arc;

use calimero_server_primitives::ws::{UnsubscribeRequest, UnsubscribeResponse};
use calimero_server_primitives::Infallible;

use crate::ws;

ws::mount_method!(UnsubscribeRequest-> Result<UnsubscribeResponse, Infallible>, handle);

async fn handle(
    request: UnsubscribeRequest,
    _state: Arc<ws::ServiceState>,
    connection_state: ws::ConnectionState,
) -> eyre::Result<UnsubscribeResponse> {
    let mut inner = connection_state.inner.write().await;
    request.context_id.iter().for_each(|id| {
        inner.subscriptions.remove(id);
    });

    Ok(UnsubscribeResponse {
        context_id: request.context_id,
    })
}
