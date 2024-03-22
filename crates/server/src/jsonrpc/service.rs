use serde::Deserialize;
use tokio::sync::oneshot;
use tracing::info;

use crate::ServerSender;

pub(crate) async fn handle_execute_method(
    sender: crate::ServerSender,
    // method: String,
    // args: Vec<u8>,
) -> eyre::Result<()> {
    // call(sender, method, args, false).await;

    Ok(())
}

pub(crate) async fn handle_read_method(
    sender: crate::ServerSender,
    // method: String,
    // args: Vec<u8>,
) -> eyre::Result<()> {
    Ok(())
}

async fn call<T>(
    sender: crate::ServerSender,
    method: String,
    args: Vec<u8>,
    writes: bool,
) -> eyre::Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let (result_sender, result_receiver) = oneshot::channel();

    sender.send((method, args, writes, result_sender)).await?;

    let outcome = result_receiver.await?;

    for log in outcome.logs {
        info!("RPC log: {}", log);
    }

    let result = serde_json::from_slice(&outcome.returns?.unwrap_or_default())?;

    Ok(result)
}
