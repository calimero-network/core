use calimero_primitives::alias::Kind;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::jsonrpc::{
    ExecuteRequest, Request, RequestId, RequestPayload, Response, ResponseBody, Version,
};
use clap::{Parser, ValueEnum};
use color_eyre::owo_colors::OwoColorize;
use const_format::concatcp;
use eyre::{bail, Result as EyreResult};
use serde_json::{json, Value};

use crate::cli::Environment;
use crate::common::{do_request, load_config, multiaddr_to_url, resolve_identifier, RequestType};
use crate::output::Report;

pub const EXAMPLES: &str = r"
  # Execute a RPC method call
  $ meroctl -- --node-name node1 call <CONTEXT_ID> <METHOD>
";

#[derive(Debug, Parser)]
#[command(about = "Executing read and write RPC calls")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct CallCommand {
    #[arg(value_name = "CONTEXT_ID", help = "ContextId or alias of the context")]
    pub context: String,

    #[arg(value_name = "METHOD", help = "Method to fetch details")]
    pub method: String,

    #[arg(long, value_parser = serde_value, help = "JSON arguments to pass to the method")]
    pub args: Option<Value>,

    #[arg(long = "as", help = "PublicKey or alias of the executor")]
    pub executor: String,

    #[arg(
        long,
        default_value = "dontcare",
        help = "Id of the JsonRpc execute call"
    )]
    pub id: Option<String>,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum CallType {
    Execute,
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

        let context_id: ContextId = resolve_identifier(&config, &self.context, Kind::Context, None)
            .await?
            .into();

        let executor: PublicKey =
            resolve_identifier(&config, &self.executor, Kind::Identity, Some(context_id))
                .await?
                .into();

        let Some(multiaddr) = config.network.server.listen.first() else {
            bail!("No address.")
        };

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
