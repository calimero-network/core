use calimero_primitives::context::ContextId;
use calimero_server_primitives::jsonrpc::{
    ExecuteRequest, Request, RequestId, RequestPayload, Response, Version,
};
use clap::{Parser, ValueEnum};
use const_format::concatcp;
use eyre::{bail, Result as EyreResult};
use serde_json::Value;

use crate::cli::Environment;
use crate::common::{do_request, load_config, multiaddr_to_url, RequestType};
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

    #[arg(
        long,
        default_value = "{}",
        help = "Arguments to the method in the app"
    )]
    pub args_json: String,

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

impl Report for Response {
    fn report(&self) {
        println!("jsonrpc: {:#?}", self.jsonrpc);
        println!("id: {:?}", self.id);
        println!("result: {:#?}", self.body);
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

        let json_payload: Value = serde_json::from_str(&self.args_json)?;
        let payload = RequestPayload::Execute(ExecuteRequest::new(
            self.context_id,
            self.method,
            json_payload,
            config
                .identity
                .public()
                .try_into_ed25519()?
                .to_bytes()
                .into(),
        ));

        let request = Request::new(
            Version::TwoPointZero,
            self.id.map(RequestId::String),
            payload,
        );

        match serde_json::to_string_pretty(&request) {
            Ok(json) => println!("Request JSON:\n{json}"),
            Err(e) => println!("Error serializing request to JSON: {e}"),
        }

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
