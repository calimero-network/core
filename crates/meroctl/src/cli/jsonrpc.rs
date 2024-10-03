use calimero_primitives::context::ContextId;
use calimero_server_primitives::jsonrpc::{
    MutateRequest, QueryRequest, Request, RequestId, RequestPayload, Version,
};
use clap::{Parser, ValueEnum};
use eyre::{bail, Result as EyreResult};
use serde_json::json;

use super::RootArgs;
use crate::common::{get_response, multiaddr_to_url, RequestType};
use crate::config_file::ConfigFile;

#[derive(Debug, Parser)]
pub struct JsonRpcCommand {
    /// Type of method call, either QUERY or MUTATE
    #[arg(long)]
    pub call_type: CallType,

    /// ContextId of the context we are using
    #[arg(long)]
    pub context_id: ContextId,

    /// Name of the method in the app
    #[arg(long)]
    pub method: String,

    /// Arguemnts to the method in the app
    #[arg(long, default_value = "{}")]
    pub args_json: String,

    /// Id of the JsonRpc call
    #[arg(long, default_value = "dontcare")]
    pub id: String,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum CallType {
    Query,
    Mutate,
}

#[expect(clippy::print_stdout, reason = "Acceptable for CLI")]
impl JsonRpcCommand {
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

        let payload = match self.call_type {
            CallType::Query => RequestPayload::Query(QueryRequest::new(
                self.context_id,
                self.method,
                json!(self.args_json),
                config
                    .identity
                    .public()
                    .try_into_ed25519()?
                    .to_bytes()
                    .into(),
            )),
            CallType::Mutate => RequestPayload::Mutate(MutateRequest::new(
                self.context_id,
                self.method,
                json!(self.args_json),
                config
                    .identity
                    .public()
                    .try_into_ed25519()?
                    .to_bytes()
                    .into(),
            )),
        };

        let request = Request::new(
            Version::TwoPointZero,
            Some(RequestId::String(self.id)),
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
