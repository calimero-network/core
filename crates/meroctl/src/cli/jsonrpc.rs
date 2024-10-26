use calimero_config::ConfigFile;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::jsonrpc::{
    ExecuteRequest, Request, RequestId, RequestPayload, Version,
};
use clap::{Parser, ValueEnum};
use const_format::concatcp;
use eyre::{bail, Result as EyreResult};
use serde_json::Value;

use super::RootArgs;
use crate::common::{get_response, multiaddr_to_url, RequestType};

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

#[expect(clippy::print_stdout, reason = "Acceptable for CLI")]
impl CallCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Config file does not exist")
        };

        let Ok(config) = ConfigFile::load(&path) else {
            bail!("Failed to load config file")
        };

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
        let response = get_response(
            &client,
            url,
            Some(request),
            &config.identity,
            RequestType::Post,
        )
        .await?;

        println!("Response: {}", response.text().await?);

        Ok(())
    }
}
