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
    request.application_ids.iter().for_each(|id| {
        inner.subscriptions.remove(id);
    });

    Ok(UnsubscribeResponse {
        application_ids: request.application_ids,
    })
}
