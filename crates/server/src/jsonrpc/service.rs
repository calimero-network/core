use eyre::Ok;
use tokio::sync::oneshot;
use tracing::info;

pub(crate) async fn handle_execute_method(
    sender: crate::ServerSender,
    params: calimero_primitives::server::JsonRpcRequestParamsCall,
) -> eyre::Result<Option<calimero_primitives::server::JsonRpcResponseResult>> {
    let args = serde_json::to_vec(&params.args_json)?;

    let result = call(sender, params.app_id, params.method, args, true).await?;

    Ok(result)
}

pub(crate) async fn handle_read_method(
    sender: crate::ServerSender,
    params: calimero_primitives::server::JsonRpcRequestParamsCall,
) -> eyre::Result<Option<calimero_primitives::server::JsonRpcResponseResult>> {
    let args = serde_json::to_vec(&params.args_json)?;

    let result = call(sender, params.app_id, params.method, args, false).await?;

    Ok(result)
}

async fn call(
    sender: crate::ServerSender,
    app_id: String,
    method: String,
    args: Vec<u8>,
    writes: bool,
) -> eyre::Result<Option<calimero_primitives::server::JsonRpcResponseResult>> {
    let (result_sender, result_receiver) = oneshot::channel();

    sender
        .send((method, app_id, args, writes, result_sender))
        .await?;

    let outcome = result_receiver.await?;

    for log in outcome.logs {
        info!("RPC log: {}", log);
    }

    match outcome.returns? {
        Some(returns) => Ok(Some(
            calimero_primitives::server::JsonRpcResponseResult::Call(String::from_utf8(returns)?),
        )),
        None => Ok(None),
    }
}
