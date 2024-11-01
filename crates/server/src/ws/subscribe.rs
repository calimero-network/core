use std::sync::Arc;

use calimero_server_primitives::ws::{SubscribeRequest, SubscribeResponse};
use calimero_server_primitives::Infallible;
use eyre::Result as EyreResult;

use crate::ws::{mount_method, ConnectionState, ServiceState};

mount_method!(SubscribeRequest-> Result<SubscribeResponse, Infallible>, handle);

async fn handle(
    request: SubscribeRequest,
    _state: Arc<ServiceState>,
    connection_state: ConnectionState,
) -> EyreResult<SubscribeResponse> {
    let mut inner = connection_state.inner.write().await;
    request.context_ids.iter().for_each(|id| {
        let _ = inner.subscriptions.insert(*id);
    });

    Ok(SubscribeResponse {
        context_ids: request.context_ids,
    })
}
