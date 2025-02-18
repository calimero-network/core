use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::ws::{Request, RequestPayload, Response, SubscribeRequest};
use clap::Parser;
use eyre::{OptionExt, Result as EyreResult};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::cli::Environment;
use crate::common::{fetch_multiaddr, load_config, multiaddr_to_url, resolve_alias};
use crate::output::{InfoLine, Report};

#[derive(Debug, Parser)]
#[command(about = "Watch events from a context")]
pub struct WatchCommand {
    /// ContextId to stream events from
    #[arg(value_name = "CONTEXT", help = "Context to stream events from")]
    pub context: Alias<ContextId>,
}

impl Report for Response {
    fn report(&self) {
        println!("id: {:?}", self.id);
        println!("payload: {:?}", self.body);
    }
}

impl WatchCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

        let multiaddr = fetch_multiaddr(&config)?;

        let context_id = resolve_alias(multiaddr, &config.identity, self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let mut url = multiaddr_to_url(multiaddr, "ws")?;
        url.set_scheme("ws")
            .map_err(|()| eyre::eyre!("Failed to set URL scheme"))?;

        environment
            .output
            .write(&InfoLine(&format!("Connecting to WebSocket at {url}")));

        let (ws_stream, _) = connect_async(url.as_str()).await?;
        let (mut write, mut read) = ws_stream.split();

        let subscribe_request = RequestPayload::Subscribe(SubscribeRequest {
            context_ids: vec![context_id],
        });
        let request = Request {
            id: None,
            payload: serde_json::to_value(&subscribe_request)?,
        };

        let subscribe_msg = serde_json::to_string(&request)?;
        write.send(WsMessage::Text(subscribe_msg)).await?;

        environment
            .output
            .write(&InfoLine(&format!("Subscribed to context {}", context_id)));
        environment
            .output
            .write(&InfoLine("Streaming events (press Ctrl+C to stop):"));

        while let Some(message) = read.next().await {
            match message {
                Ok(msg) => {
                    if let WsMessage::Text(text) = msg {
                        let response = serde_json::from_str::<Response>(&text)?;
                        environment.output.write(&response);
                    }
                }
                Err(err) => eprintln!("Error receiving message: {err}"),
            }
        }

        Ok(())
    }
}
