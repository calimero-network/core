use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::jsonrpc::{
    ExecutionRequest, Request, RequestId, RequestPayload, Version,
};
use clap::Parser;
use const_format::concatcp;
use eyre::{OptionExt, Result};
use serde_json::{json, Value};

use crate::cli::validation::non_empty_string;
use crate::cli::Environment;

pub const EXAMPLES: &str = r"
  # Execute a RPC method call
  $ meroctl --node node1 call <CONTEXT> <METHOD>
";

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

    #[arg(value_name = "METHOD", help = "The method to call", value_parser = non_empty_string)]
    pub method: String,

    #[arg(long, value_parser = serde_value, help = "JSON arguments to pass to the method")]
    pub args: Option<Value>,

    #[arg(
        long = "as",
        help = "The identity of the executor",
        default_value = "default"
    )]
    pub executor: Alias<PublicKey>,

    #[arg(long, help = "Id of the JsonRpc call")]
    pub id: Option<String>,

    #[arg(
        long = "substitute",
        help = "Comma-separated list of aliases to substitute in the payload (use {alias} in payload)",
        value_name = "ALIAS",
        value_delimiter = ','
    )]
    pub substitute: Vec<Alias<PublicKey>>,
}

fn serde_value(s: &str) -> serde_json::Result<Value> {
    serde_json::from_str(s)
}

impl CallCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let resolve_response = client.resolve_alias(self.context, None).await?;
        let context_id = resolve_response
            .value()
            .cloned()
            .ok_or_eyre("Failed to resolve context: no value found")?;

        let executor = client
            .resolve_alias(self.executor, Some(context_id))
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let payload = RequestPayload::Execute(ExecutionRequest::new(
            context_id,
            self.method,
            self.args.unwrap_or(json!({})),
            executor,
            self.substitute,
        ));

        let request = Request::new(
            Version::TwoPointZero,
            self.id.map(RequestId::String).unwrap_or_default(),
            payload,
        );

        let response = client.execute_jsonrpc(request).await?;

        // Debug: Print what we're about to output
        eprintln!(
            "🔍 meroctl call output: {}",
            serde_json::to_string_pretty(&response)?
        );

        environment.output.write(&response);

        Ok(())
    }
}
