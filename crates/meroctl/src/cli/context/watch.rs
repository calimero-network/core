use std::borrow::Cow;
use std::process::Stdio;

use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::sse::{
    ContextIds, Request as SubscriptionRequest, RequestPayload, Response, ResponseBody, SseEvent,
};
use clap::Parser;
use eyre::Result;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::cli::Environment;
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
        println!("Received response: {:?}", self);
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
        println!("Command executed: {}", self.cmd.join(" "));
        if let Some(status) = self.status {
            println!("Exit status: {}", status);
        }
        if !self.stdout.is_empty() {
            println!("Stdout: {}", self.stdout);
        }
        if !self.stderr.is_empty() {
            println!("Stderr: {}", self.stderr);
        }
    }
}

impl WatchCommand {
    pub async fn run(self, environment: &Environment) -> Result<()> {
        let client = environment.client()?;

        // let resolve_response = client.resolve_alias(self.context, None).await?;
        // let context_id = resolve_response
        //     .value()
        //     .copied()
        //     .ok_or_eyre("unable to resolve")?;
        //
        //
        let in_value = [0; 32];

        let context_id = ContextId::from(in_value);
        let mut url = client.api_url().clone();
        url.set_path("sse");

        environment
            .output
            .write(&InfoLine(&format!("Connecting to {url}")));

        let response = client.stream_sse().await?;
        let status = response.status();

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(eyre::eyre!("HTTP {}: {}", status, body));
        }

        if let Some(cmd) = &self.exec {
            environment.output.write(&InfoLine(&format!(
                "Will execute command: {}",
                cmd.join(" ")
            )));
        }

        environment
            .output
            .write(&InfoLine("Streaming events (press Ctrl+C to stop):"));

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut event_count = 0usize;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buffer.find("\n\n") {
                let block = buffer.drain(..pos + 2).collect::<String>();

                let mut event_type = SseEvent::Message;
                let mut data_str = String::new();

                for line in block.lines() {
                    if line.is_empty() || line.starts_with(':') {
                        // keep-alive
                        continue;
                    }
                    if let Some(rest) = line.strip_prefix("event:") {
                        event_type = match rest.trim() {
                            "connect" => SseEvent::Connect,
                            "message" => SseEvent::Message,
                            "close" => SseEvent::Close,
                            "error" => SseEvent::Error,
                            other => {
                                eprintln!("Unknown event type: {other}, defaulting to message");
                                SseEvent::Message
                            }
                        };
                    } else if let Some(rest) = line.strip_prefix("data:") {
                        data_str.push_str(rest.trim());
                    }
                }

                match event_type {
                    SseEvent::Message => {
                        if data_str.is_empty() || data_str.starts_with(':') {
                            continue; // keep-alive
                        }

                        let response: Response = serde_json::from_str(&data_str)?;
                        environment.output.write(&response);

                        if let Some(cmd) = &self.exec {
                            if let Some(max) = self.count {
                                if event_count >= max {
                                    break;
                                }
                            }

                            let mut child = Command::new(&cmd[0])
                                .args(&cmd[1..])
                                .stdin(Stdio::piped())
                                .spawn()?;

                            let stdin = child.stdin.take();

                            let stdin = tokio::spawn(async move {
                                let Some(mut stdin) = stdin else {
                                    return Ok(());
                                };

                                if let ResponseBody::Result(result) = &response.body {
                                    return stdin.write_all(result.to_string().as_bytes()).await;
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
                    SseEvent::Close => {
                        environment
                            .output
                            .write(&InfoLine("SSE stream closed by server."));
                        return Ok(());
                    }
                    SseEvent::Error => {
                        environment
                            .output
                            .write(&ErrorLine(&format!("SSE error: {data_str}")));
                    }
                    SseEvent::Connect => {
                        let request = SubscriptionRequest {
                            id: data_str,
                            payload: RequestPayload::Subscribe(ContextIds {
                                context_ids: vec![context_id],
                            }),
                        };
                        let response = client.subscribe_context(request).await?;
                        if !status.is_success() {
                            return Err(eyre::eyre!("HTTP {}: {:?}", status, response.body));
                        }

                        environment
                            .output
                            .write(&InfoLine(&format!("Subscribed to context {}", context_id)));
                    }
                }
            }
        }

        Ok(())
    }
}
