use std::borrow::Cow;
use std::process::Stdio;

use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::ws::{
    Request, RequestPayload, Response, ResponseBody, SubscribeRequest,
};
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::{OptionExt, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::cli::Environment;
use crate::common::resolve_alias;
use crate::output::{ErrorLine, InfoLine, Report};

pub const EXAMPLES: &str = r#"
  # Watch events from default context
  $ meroctl context watch

  # Watch events and show notification
  $ meroctl context watch -x notify-send "New event"

  # Watch events and log to file (first 10 events)
  $ meroctl context watch -x sh -c "echo 'Event received' >> events.log" -n 10

  # Watch events and run custom script with arguments
  $ meroctl context watch -x ./my-script.sh --arg1 value1
"#;

#[derive(Debug, Parser)]
#[command(after_help = EXAMPLES)]
#[command(about = "Watch events from a context and optionally execute commands")]
pub struct WatchCommand {
    /// ContextId to stream events from
    #[arg(
        value_name = "CONTEXT",
        help = "Context to stream events from",
        default_value = "default"
    )]
    pub context: Alias<ContextId>,

    /// Command to execute when an event is received (can specify multiple args)
    #[arg(short = 'x', long, value_name = "COMMAND", num_args = 1..)]
    pub exec: Option<Vec<String>>,

    /// Maximum number of events to process before exiting
    #[arg(short = 'n', long, value_name = "COUNT")]
    pub count: Option<usize>,
}

impl Report for Response {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("WebSocket Response").fg(Color::Blue)]);

        let _ = table.add_row(vec![format!("ID: {:?}", self.id)]);

        match &self.body {
            ResponseBody::Result(value) => {
                let _ = table.add_row(vec![format!("Result: {:#}", value)]);
            }
            ResponseBody::Error(error) => {
                let _ = table.add_row(vec![format!("Error: {:?}", error)]);
            }
        }

        println!("{table}");
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct ExecutionOutput<'a> {
    #[serde(borrow)]
    cmd: Cow<'a, [String]>,
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Report for ExecutionOutput<'_> {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.add_row(vec![format!("Command: {}", self.cmd.join(" "))]);
        let _ = table.add_row(vec![format!("Status: {:?}", self.status)]);
        let _ = table.add_row(vec![format!("Stdout: {}", self.stdout)]);
        let _ = table.add_row(vec![format!("Stderr: {}", self.stderr)]);

        println!("{table}");
    }
}

impl WatchCommand {
    pub async fn run(self, environment: &Environment) -> Result<()> {
        let connection = environment.connection();

        let resolve_response = resolve_alias(connection, self.context, None).await?;

        let context_id = resolve_response
            .value()
            .cloned()
            .ok_or_eyre("Failed to resolve context: no value found")?;

        let mut url = connection.api_url.clone();

        let scheme = match url.scheme() {
            "https" => "wss",
            "http" | _ => "ws",
        };

        url.set_scheme(scheme)
            .map_err(|()| eyre::eyre!("Failed to set URL scheme"))?;
        url.set_path("ws");

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
            environment.output.write(&InfoLine(&format!(
                "Will execute command: {}",
                cmd.join(" ")
            )));
        }

        environment
            .output
            .write(&InfoLine("Streaming events (press Ctrl+C to stop):"));

        let mut event_count = 0;
        while let Some(message) = read.next().await {
            match message {
                Ok(msg) => {
                    if let WsMessage::Text(text) = msg {
                        let response = serde_json::from_str::<Response>(&text)?;
                        environment.output.write(&response);

                        if let Some(cmd) = &self.exec {
                            if let Some(max_count) = self.count {
                                if event_count >= max_count {
                                    break;
                                }
                            }

                            let mut child = Command::new(&cmd[0])
                                .args(&cmd[1..])
                                .stdin(Stdio::piped())
                                .spawn()?;

                            let stdin = child.stdin.take();

                            let stdin = tokio::spawn(async {
                                let Some(mut stdin) = stdin else {
                                    return Ok(());
                                };

                                if let ResponseBody::Result(result) = response.body {
                                    let result = result.to_string();

                                    return stdin.write_all(result.as_bytes()).await;
                                }

                                Ok(())
                            });

                            let output = child
                                .wait_with_output()
                                .await
                                .map_err(|e| eyre::eyre!("Failed to execute command: {}", e))?;

                            stdin.await??;

                            let outcome = ExecutionOutput {
                                cmd: cmd.into(),
                                status: output.status.code(),
                                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                            };

                            environment.output.write(&outcome);
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
