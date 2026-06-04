use std::io::Write as _;
use std::time::Duration;

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

    #[arg(
        long,
        value_name = "SECONDS",
        default_value_t = 120,
        help = "Per-call timeout for the interactive shell, in seconds (0 disables)"
    )]
    pub timeout: u64,
}

fn serde_value(s: &str) -> serde_json::Result<Value> {
    serde_json::from_str(s)
}

impl CallCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        if self.interactive {
            let timeout = (self.timeout > 0).then(|| Duration::from_secs(self.timeout));
            return run_shell(
                environment,
                client,
                self.context,
                self.substitute,
                self.method,
                self.args,
                timeout,
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
    timeout: Option<Duration>,
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
    // any `--args` given alongside it. A dead socket here means the shell can't
    // start, so a transport failure ends it.
    if let Some(method) = seed_method {
        let args = seed_args.unwrap_or_else(|| json!({}));
        if let CallOutcome::Closed(err) =
            run_call(&mut session, timeout, context_id, &substitute, method, args).await
        {
            eprintln!("Connection closed: {err}");
            return Ok(());
        }
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
                match result {
                    // `keepalive` returns `Infallible` on success, so `Ok` is
                    // unreachable — the match proves the shell never exits silently.
                    Ok(never) => match never {},
                    Err(err) => eprintln!("Connection closed: {err}"),
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

        match run_call(&mut session, timeout, context_id, &substitute, method, args).await {
            CallOutcome::Done => {}
            CallOutcome::TimedOut(dur) => eprintln!(
                "No response within {}s; the call may still be running on the node.",
                dur.as_secs()
            ),
            CallOutcome::Closed(err) => {
                eprintln!("Connection closed: {err}");
                break 'shell;
            }
        }
    }

    Ok(())
}

/// What became of one shell call. A handler-level failure is printed inside
/// `run_call` (the shell stays alive); the variants here are the outcomes the
/// caller must act on.
enum CallOutcome {
    /// Completed — the result (or a handler error) was printed.
    Done,
    /// No reply arrived within the per-call timeout. The session is still
    /// usable, so the shell keeps going.
    TimedOut(Duration),
    /// The socket errored or closed; the session is dead and the shell ends.
    Closed(eyre::Report),
}

/// Run a single call within the shell, bounding the wait by `timeout` so a
/// server that never replies can't hang the prompt indefinitely.
async fn run_call(
    session: &mut WsSession,
    timeout: Option<Duration>,
    context_id: ContextId,
    substitute: &[Alias<PublicKey>],
    method: String,
    args: Value,
) -> CallOutcome {
    let request = ExecutionRequest::new(context_id, method, args, substitute.to_vec());

    let result = match timeout {
        Some(dur) => match tokio::time::timeout(dur, session.execute(request)).await {
            Ok(result) => result,
            Err(_elapsed) => return CallOutcome::TimedOut(dur),
        },
        None => session.execute(request).await,
    };

    match result {
        Ok(response) => {
            print_response(&response);
            CallOutcome::Done
        }
        Err(err) => CallOutcome::Closed(err),
    }
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
