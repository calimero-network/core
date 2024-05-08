use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{QueryError, QueryRequest, QueryResponse};

use crate::jsonrpc;

jsonrpc::mount_method!(QueryRequest-> Result<QueryResponse, QueryError>, handle);

async fn handle(
    request: QueryRequest,
    state: Arc<jsonrpc::ServiceState>,
) -> eyre::Result<QueryResponse> {
    let args = match serde_json::to_vec(&request.args_json) {
        Ok(args) => args,
        Err(err) => {
            eyre::bail!(QueryError::SerdeError {
                message: err.to_string()
            })
        }
    };

    match jsonrpc::call(
        state.server_sender.clone(),
        request.application_id,
        request.method,
        args,
        false,
    )
    .await
    {
        Ok(Some(output)) => match serde_json::from_str::<serde_json::Value>(&output) {
            Ok(v) => Ok(QueryResponse { output: Some(v) }),
            Err(err) => eyre::bail!(QueryError::SerdeError {
                message: err.to_string()
            }),
        },
        Ok(None) => Ok(QueryResponse { output: None }),
        Err(err) => match err.downcast::<calimero_node_primitives::CallError>() {
            Ok(err) => eyre::bail!(QueryError::CallError(err)),
            Err(err) => eyre::bail!(err),
        },
    }
}
