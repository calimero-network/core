use std::io::Write as _;

use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::jsonrpc::{
    ExecutionRequest, Request, RequestId, RequestPayload, Response, ResponseBody,
    ResponseBodyResult, Version,
};
use clap::Parser;
use const_format::concatcp;
use eyre::{OptionExt, Result};
use serde_json::{json, Value};
use tokio::io::{stdin, AsyncBufReadExt, BufReader};

use crate::cli::validation::non_empty_string;
use crate::cli::Environment;
use crate::client::Client;
use crate::output::InfoLine;
use crate::ws::WsSession;

pub const EXAMPLES: &str = r#"
  # Call a mutation (e.g. add_item, set) on a context
  $ meroctl --node <NODE_ID> call <METHOD_NAME> \
    --context <CONTEXT_ID> \
    --args '<ARGS_JSON>'

  # Call a view (e.g. get_item, get) on a context
  $ meroctl --node <NODE_ID> call <METHOD_NAME> \
    --context <CONTEXT_ID> \
    --args '<ARGS_JSON>' \
    --as <IDENTITY_PUBLIC_KEY>

  # Open an interactive shell over a single persistent WebSocket and run
  # many calls without re-connecting on each one
  $ meroctl --node <NODE_ID> call -i --context <CONTEXT_ID>
"#;

#[derive(Debug, Parser)]
#[command(about = "Call a method on a context")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct CallCommand {
    #[arg(long, short)]
    #[arg(
        value_name = "CONTEXT",
        help = "The context to call the method on",
        default_value = "default"
    )]
    pub context: Alias<ContextId>,

    #[arg(value_name = "METHOD", help = "The method to call (omit with --interactive)", value_parser = non_empty_string)]
    pub method: Option<String>,

    #[arg(long, value_parser = serde_value, help = "JSON arguments to pass to the method")]
    pub args: Option<Value>,

    #[arg(long, help = "Id of the JsonRpc call")]
    pub id: Option<String>,

    #[arg(
        long = "substitute",
        help = "Comma-separated list of aliases to substitute in the payload (use {alias} in payload)",
        value_name = "ALIAS",
        value_delimiter = ','
    )]
    pub substitute: Vec<Alias<PublicKey>>,

    #[arg(
        long,
        short,
        help = "Open an interactive shell that keeps one WebSocket open and runs many calls through it"
    )]
    pub interactive: bool,
}

fn serde_value(s: &str) -> serde_json::Result<Value> {
    serde_json::from_str(s)
}

impl CallCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        if self.interactive {
            return run_shell(
                environment,
                client,
                self.context,
                self.substitute,
                self.method,
                self.args,
            )
            .await;
        }

        let method = self
            .method
            .ok_or_eyre("a METHOD is required (or use --interactive for a shell)")?;

        let context_id = resolve_context(client, self.context).await?;

        let payload = RequestPayload::Execute(ExecutionRequest::new(
            context_id,
            method,
            self.args.unwrap_or(json!({})),
            self.substitute,
        ));

        let request = Request::new(
            Version::TwoPointZero,
            self.id.map(RequestId::String).unwrap_or_default(),
            payload,
        );

        let response = client.execute_jsonrpc(request).await?;

        environment.output.write(&response);

        Ok(())
    }
}

async fn resolve_context(client: &Client, context: Alias<ContextId>) -> Result<ContextId> {
    client
        .resolve_alias(context, None)
        .await?
        .value()
        .copied()
        .ok_or_eyre("Failed to resolve context: no value found")
}

/// Interactive shell over a single persistent WebSocket.
///
/// Reads `<method> [args-json]` lines from stdin, runs each over the same
/// `execute` session, and prints the result. A WebSocket (vs. per-call HTTP)
/// pays off precisely because the connection is reused across calls here.
async fn run_shell(
    environment: &Environment,
    client: &Client,
    mut context: Alias<ContextId>,
    substitute: Vec<Alias<PublicKey>>,
    seed_method: Option<String>,
    seed_args: Option<Value>,
) -> Result<()> {
    let mut context_id = resolve_context(client, context).await?;
    let mut session = WsSession::connect(client).await?;

    environment.output.write(&InfoLine(&format!(
        "Connected over WebSocket to {}",
        client.ws_url()?
    )));
    eprintln!("Context: {context} ({context_id})");
    eprintln!("Enter `<method> [args-json]`, e.g. `set {{\"key\":\"k\",\"value\":\"v\"}}`.");
    eprintln!("Meta-commands: :context <alias>, :help, :quit (or Ctrl-D to exit).");

    // A method passed on the command line runs as the first call, honouring
    // any `--args` given alongside it.
    if let Some(method) = seed_method {
        let args = seed_args.unwrap_or_else(|| json!({}));
        run_call(&mut session, context_id, &substitute, method, args).await?;
    }

    let mut lines = BufReader::new(stdin()).lines();
    'shell: loop {
        eprint!("{context}> ");
        let _ = std::io::stderr().flush();

        // Wait for a command, but keep servicing the socket (answering pings)
        // while idle so the node doesn't drop a connection that sat at the
        // prompt longer than its ping timeout. `keepalive` only resolves when
        // the socket errors or closes; treat that as a clean end of session
        // with a one-line message rather than an error trace.
        let next_line = tokio::select! {
            line = lines.next_line() => line?,
            result = session.keepalive() => {
                if let Err(err) = result {
                    eprintln!("Connection closed: {err}");
                }
                break 'shell;
            }
        };

        let Some(line) = next_line else {
            eprintln!();
            break; // Ctrl-D / end of input
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix(':') {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let cmd = parts.next().unwrap_or_default();
            let arg = parts.next().map(str::trim).unwrap_or_default();

            match cmd {
                "quit" | "q" | "exit" => break,
                "help" | "h" => print_help(),
                "context" | "ctx" => match arg.parse::<Alias<ContextId>>() {
                    Ok(alias) => match resolve_context(client, alias).await {
                        Ok(id) => {
                            context = alias;
                            context_id = id;
                            eprintln!("Context: {context} ({context_id})");
                        }
                        Err(err) => eprintln!("error: {err}"),
                    },
                    Err(err) => eprintln!("error: invalid context alias: {err}"),
                },
                other => eprintln!("error: unknown command `:{other}` (try `:help`)"),
            }
            continue;
        }

        let mut parts = line.splitn(2, char::is_whitespace);
        let method = parts.next().unwrap_or_default().to_owned();
        let args = match parts.next().map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw) => match serde_json::from_str::<Value>(raw) {
                Ok(value) => value,
                Err(err) => {
                    eprintln!("error: args must be valid JSON: {err}");
                    continue;
                }
            },
            None => json!({}),
        };

        run_call(&mut session, context_id, &substitute, method, args).await?;
    }

    Ok(())
}

/// Run a single call within the shell and print its outcome. A handler-level
/// failure comes back as an error body (printed, shell continues); only a
/// transport/protocol failure propagates and ends the session.
async fn run_call(
    session: &mut WsSession,
    context_id: ContextId,
    substitute: &[Alias<PublicKey>],
    method: String,
    args: Value,
) -> Result<()> {
    let request = ExecutionRequest::new(context_id, method, args, substitute.to_vec());
    let response = session.execute(request).await?;
    print_response(&response);
    Ok(())
}

fn print_response(response: &Response) {
    match &response.body {
        ResponseBody::Result(ResponseBodyResult(value)) => {
            let rendered =
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
            println!("{rendered}");
        }
        ResponseBody::Error(error) => {
            let rendered =
                serde_json::to_string_pretty(error).unwrap_or_else(|_| "<error>".to_owned());
            eprintln!("error: {rendered}");
        }
    }
}

fn print_help() {
    eprintln!("Commands:");
    eprintln!("  <method> [args-json]   run a method (args default to {{}})");
    eprintln!("  :context <alias>       switch the active context");
    eprintln!("  :help                  show this help");
    eprintln!("  :quit                  exit (or press Ctrl-D)");
}
