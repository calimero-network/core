use std::sync::Arc;

use calimero_server_primitives::ws::{SubscribeRequest, SubscribeResponse};
use calimero_server_primitives::Infallible;

use crate::ws;

ws::mount_method!(SubscribeRequest-> Result<SubscribeResponse, Infallible>, handle);

async fn handle(
    request: SubscribeRequest,
    _state: Arc<ws::ServiceState>,
    connection_state: ws::ConnectionState,
) -> eyre::Result<SubscribeResponse> {
    let mut inner = connection_state.inner.write().await;
    request.context_ids.iter().for_each(|id| {
        inner.subscriptions.insert(*id);
    });

    Ok(SubscribeResponse {
        context_ids: request.context_ids,
    })
}
