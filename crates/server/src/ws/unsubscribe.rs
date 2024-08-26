use std::sync::Arc;

use calimero_server_primitives::ws::{UnsubscribeRequest, UnsubscribeResponse};
use calimero_server_primitives::Infallible;
use eyre::Result as EyreResult;

use crate::ws::{mount_method, ConnectionState, ServiceState};

mount_method!(UnsubscribeRequest-> Result<UnsubscribeResponse, Infallible>, handle);

async fn handle(
    request: UnsubscribeRequest,
    _state: Arc<ServiceState>,
    connection_state: ConnectionState,
) -> EyreResult<UnsubscribeResponse> {
    let mut inner = connection_state.inner.write().await;
    request.context_ids.iter().for_each(|id| {
        let _ = inner.subscriptions.remove(id);
    });

    Ok(UnsubscribeResponse::new(request.context_ids))
}
