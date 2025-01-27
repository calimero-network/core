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
use crate::common::{do_request, load_config, multiaddr_to_url, RequestType};
use crate::identity::open_identity;
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
    #[arg(value_name = "CONTEXT_ID", help = "ContextId of the context")]
    pub context_id: ContextId,

    #[arg(value_name = "METHOD", help = "Method to fetch details")]
    pub method: String,

    #[arg(long, value_parser = serde_value, help = "JSON arguments to pass to the method")]
    pub args: Option<Value>,

    #[arg(
        long = "as",
        help = "Public key of the executor",
        conflicts_with = "identity_name"
    )]
    pub executor: Option<PublicKey>,

    #[clap(
        short = 'i',
        long,
        value_name = "IDENTITY_NAME",
        help = "Name of the identity which you want to use as executor"
    )]
    identity_name: Option<String>,

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

        let Some(multiaddr) = config.network.server.listen.first() else {
            bail!("No address.")
        };

        let url = multiaddr_to_url(multiaddr, "jsonrpc/dev")?;

        let public_key = match self.executor {
            Some(public_key) => public_key,
            None => open_identity(environment, self.identity_name.as_ref().unwrap())?.public_key,
        };

        let payload = RequestPayload::Execute(ExecuteRequest::new(
            self.context_id,
            self.method,
            self.args.unwrap_or(json!({})),
            public_key,
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
