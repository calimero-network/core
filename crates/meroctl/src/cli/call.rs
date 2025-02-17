use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::jsonrpc::{
    ExecuteRequest, Request, RequestId, RequestPayload, Response, ResponseBody, Version,
};
use clap::Parser;
use color_eyre::owo_colors::OwoColorize;
use const_format::concatcp;
use eyre::{OptionExt, Result as EyreResult};
use serde_json::{json, Value};

use crate::cli::Environment;
use crate::common::{
    do_request, fetch_multiaddr, load_config, multiaddr_to_url, resolve_alias, RequestType,
};
use crate::output::Report;

pub const EXAMPLES: &str = r"
  # Execute a RPC method call
  $ meroctl -- --node-name node1 call <CONTEXT> <METHOD>
";

#[derive(Debug, Parser)]
#[command(about = "Call a method on a context")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct CallCommand {
    #[arg(value_name = "CONTEXT", help = "The context to call the method on")]
    pub context: Alias<ContextId>,

    #[arg(value_name = "METHOD", help = "The method to call")]
    pub method: String,

    #[arg(long, value_parser = serde_value, help = "JSON arguments to pass to the method")]
    pub args: Option<Value>,

    #[arg(long = "as", help = "The identity of the executor")]
    pub executor: Alias<PublicKey>,

    #[arg(long, default_value = "dontcare", help = "Id of the JsonRpc call")]
    pub id: Option<String>,
}

fn serde_value(s: &str) -> serde_json::Result<Value> {
    serde_json::from_str(s)
}

impl Report for Response {
    fn report(&self) {
        match &self.body {
            ResponseBody::Result(result) => {
                println!("return value:");
                let result = format!(
                    "(json): {}",
                    format!("{:#}", result.0)
                        .lines()
                        .map(|line| line.cyan().to_string())
                        .collect::<Vec<_>>()
                        .join("\n")
                );

                for line in result.lines() {
                    println!("  > {line}");
                }
            }
            ResponseBody::Error(error) => {
                println!("{error}");
            }
        }
    }
}

#[expect(clippy::print_stdout, reason = "Acceptable for CLI")]
impl CallCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

        let multiaddr = fetch_multiaddr(&config)?;

        let context_id = resolve_alias(multiaddr, &config.identity, self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let executor = resolve_alias(multiaddr, &config.identity, self.executor, Some(context_id))
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let url = multiaddr_to_url(multiaddr, "jsonrpc/dev")?;

        let payload = RequestPayload::Execute(ExecuteRequest::new(
            context_id,
            self.method,
            self.args.unwrap_or(json!({})),
            executor,
        ));

        let request = Request::new(
            Version::TwoPointZero,
            self.id.map(RequestId::String),
            payload,
        );

        let client = reqwest::Client::new();
        let response: Response = do_request(
            &client,
            url,
            Some(request),
            &config.identity,
            RequestType::Post,
        )
        .await?;

        environment.output.write(&response);

        Ok(())
    }
}
