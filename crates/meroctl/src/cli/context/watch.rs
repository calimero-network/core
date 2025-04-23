use std::process::Command;

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
use crate::output::{ErrorLine, InfoLine, Report};

#[derive(Debug, Parser)]
#[command(about = "Watch events from a context and optionally execute commands")]
pub struct WatchCommand {
    /// ContextId to stream events from
    #[arg(
        value_name = "CONTEXT",
        help = "Context to stream events from",
        default_value = "default"
    )]
    pub context: Alias<ContextId>,

    /// Command to execute when an event is received
    #[arg(short, long, value_name = "COMMAND")]
    pub exec: Option<String>,

    /// Maximum number of events to process before exiting
    #[arg(short, long, value_name = "COUNT")]
    pub count: Option<usize>,
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

        let resolve_response =
            resolve_alias(multiaddr, &config.identity, self.context, None).await?;

        let context_id = resolve_response
            .value()
            .cloned()
            .ok_or_eyre("Failed to resolve context: no value found")?;

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

        if let Some(cmd) = &self.exec {
            environment
                .output
                .write(&InfoLine(&format!("Will execute command: {}", cmd)));
        }

        environment
            .output
            .write(&InfoLine("Streaming events (press Ctrl+C to stop):"));

        let mut event_count = 0;
        while let Some(message) = read.next().await {
            if let Some(max_count) = self.count {
                if event_count >= max_count {
                    break;
                }
            }

            match message {
                Ok(msg) => {
                    if let WsMessage::Text(text) = msg {
                        let response = serde_json::from_str::<Response>(&text)?;
                        environment.output.write(&response);

                        if let Some(cmd) = &self.exec {
                            let output = Command::new("sh")
                                .arg("-c")
                                .arg(cmd)
                                .output()
                                .map_err(|e| eyre::eyre!("Failed to execute command: {}", e))?;

                            if !output.status.success() {
                                environment.output.write(&ErrorLine(&format!(
                                    "Command failed: {}",
                                    String::from_utf8_lossy(&output.stderr)
                                )));
                            } else {
                                environment.output.write(&InfoLine(&format!(
                                    "Command output: {}",
                                    String::from_utf8_lossy(&output.stdout)
                                )));
                            }
                        }

                        event_count += 1;
                    }
                }
                Err(err) => {
                    environment
                        .output
                        .write(&ErrorLine(&format!("Error receiving message: {err}")));
                }
            }
        }

        Ok(())
    }
}
