use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::jsonrpc::{
    ExecutionRequest, ExecutionResponse, Request, RequestId, RequestPayload, Response,
    ResponseBody, ResponseBodyResult, Version,
};
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use const_format::concatcp;
use eyre::{OptionExt, Result};
use serde_json::{json, Value};

use crate::cli::Environment;
use crate::common::resolve_alias;
use crate::output::Report;

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

    #[arg(value_name = "METHOD", help = "The method to call")]
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

impl Report for Response {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("RPC Response").fg(Color::Blue)]);

        match &self.body {
            ResponseBody::Result(ResponseBodyResult(result)) => {
                if let Ok(result) = serde_json::from_value::<ExecutionResponse>(result.clone()) {
                    if let Some(output) = &result.output {
                        let _ = table.add_row(vec![format!("Output: {:#}", output)]);
                    } else {
                        let _ = table.add_row(vec!["<no output>".to_string()]);
                    }
                } else {
                    let _ = table.add_row(vec![format!("Result: {:#}", result)]);
                }
            }
            ResponseBody::Error(error) => {
                let _ = table.add_row(vec![format!("Error: {}", error)]);
            }
        }
        println!("{table}");
    }
}

impl CallCommand {
    pub async fn run(self, environment: &Environment) -> Result<()> {
        let connection = environment.connection()?;

        let resolve_response = resolve_alias(connection, self.context, None).await?;
        let context_id = resolve_response
            .value()
            .cloned()
            .ok_or_eyre("Failed to resolve context: no value found")?;

        let executor = resolve_alias(connection, self.executor, Some(context_id))
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

        let response: Response = connection.post("jsonrpc", request).await?;
        environment.output.write(&response);

        Ok(())
    }
}
