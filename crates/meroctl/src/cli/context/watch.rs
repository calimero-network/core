use calimero_primitives::context::ContextId;
use calimero_server_primitives::ws::{RequestPayload, SubscribeRequest};
use clap::Parser;
use eyre::Result as EyreResult;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;

use super::RootArgs;
use crate::common::{fetch_multiaddr, load_config, multiaddr_to_url};

#[derive(Debug, Parser)]
pub struct WatchCommand {
    /// ContextId to stream events from
    #[arg(long)]
    pub context_id: ContextId,
}

impl WatchCommand {
    pub async fn run(self, args: RootArgs) -> EyreResult<()> {
        let config = load_config(&args.node_name)?;

        let mut url = multiaddr_to_url(fetch_multiaddr(&config)?, "ws")?;
        url.set_scheme("ws")
            .map_err(|_| eyre::eyre!("Failed to set URL scheme"))?;

        println!("Connecting to WebSocket at {}", url);

        let (ws_stream, _) = connect_async(url.as_str()).await?;

        let (mut write, mut read) = ws_stream.split();

        // Send subscribe message
        let subscribe_request =
            RequestPayload::Subscribe(SubscribeRequest::new(vec![self.context_id]));
        let subscribe_msg = serde_json::to_string(&subscribe_request)?;
        write
            .send(tokio_tungstenite::tungstenite::Message::Text(subscribe_msg))
            .await?;

        println!("Subscribed to context {}", self.context_id);
        println!("Streaming events (press Ctrl+C to stop):");

        while let Some(message) = read.next().await {
            match message {
                Ok(msg) => {
                    if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                        println!("{}", text);
                    }
                }
                Err(e) => eprintln!("Error receiving message: {}", e),
            }
        }

        Ok(())
    }
}
