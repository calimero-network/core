use calimero_primitives::context::ContextId;
use calimero_server_primitives::ws::{RequestPayload, SubscribeRequest};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use serde_json::from_str;
use tokio_tungstenite::connect_async;

use super::RootArgs;
use crate::common::{fetch_multiaddr, load_config, multiaddr_to_url, CliError};

#[derive(Debug, Parser)]
pub struct WatchCommand {
    /// ContextId to stream events from
    #[arg(long)]
    pub context_id: ContextId,
}

#[allow(dependency_on_unit_never_type_fallback)]
impl WatchCommand {
    pub async fn run(self, args: RootArgs) -> Result<(), CliError> {
        let config = load_config(&args.home, &args.node_name)?;

        let mut url = multiaddr_to_url(fetch_multiaddr(&config)?, "ws")?;
        url.set_scheme("ws")
            .map_err(|_| CliError::InternalError(format!("Failed to set scheme as ws")))?;

        println!("Connecting to WebSocket at {}", url);

        let (ws_stream, _) = connect_async(url.as_str())
            .await
            .map_err(|e| CliError::MethodCallError(e.to_string()))?;

        let (mut write, mut read) = ws_stream.split();

        // Send subscribe message
        let subscribe_request =
            RequestPayload::Subscribe(SubscribeRequest::new(vec![self.context_id]));
        let subscribe_msg = serde_json::to_string(&subscribe_request)
            .map_err(|e| CliError::InternalError(e.to_string()))?;
        write
            .send(tokio_tungstenite::tungstenite::Message::Text(subscribe_msg))
            .await
            .map_err(|e| CliError::MethodCallError(e.to_string()))?;

        while let Some(message) = read.next().await {
            match message {
                Ok(msg) => {
                    if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                        println!(
                            "{:#?}",
                            from_str(&text)
                                .map_err(|err| CliError::InternalError(err.to_string()))?
                        )
                    }
                }
                Err(e) => println!(
                    "{:#?}",
                    from_str(&e.to_string())
                        .map_err(|err| CliError::InternalError(err.to_string()))?
                ),
            }
        }

        Ok(())
    }
}
