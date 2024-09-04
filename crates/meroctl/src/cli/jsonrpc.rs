use calimero_primitives::context::ContextId;
use calimero_server_primitives::jsonrpc::{
    MutateRequest, QueryRequest, Request, RequestId, RequestPayload, Version,
};
use clap::{value_parser, Parser};
use eyre::{bail, Result as EyreResult};
use serde_json::Value;

use super::RootArgs;
use crate::common::{get_response, multiaddr_to_url};
use crate::config_file::ConfigFile;

#[derive(Debug, Parser)]
pub struct JsonRpcCommand {
    #[arg(long)]
    pub call_type: String,

    #[arg(long)]
    pub context_id: ContextId,

    #[arg(long)]
    pub method: String,

    #[arg(long, value_parser = value_parser!(Value), default_value = "{}")]
    pub args_json: Value,

    #[arg(long, default_value = "dontcare")]
    pub id: String,
}

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

        let payload = match self.call_type.to_lowercase().as_str() {
            "query" => RequestPayload::Query(QueryRequest::new(
                self.context_id,
                self.method,
                self.args_json,
                config.identity.public().try_into_ed25519()?.to_bytes(),
            )),
            "mutate" => RequestPayload::Mutate(MutateRequest::new(
                self.context_id,
                self.method,
                self.args_json,
                config.identity.public().try_into_ed25519()?.to_bytes(),
            )),
            _ => bail!("Invalid call_type. Must be either 'query' or 'mutate'."),
        };

        let request = Request {
            jsonrpc: Version::TwoPointZero,
            id: Some(RequestId::String(self.id)),
            payload,
        };

        match serde_json::to_string_pretty(&request) {
            Ok(json) => println!("Request JSON:\n{}", json),
            Err(e) => println!("Error serializing request to JSON: {}", e),
        }

        let client = reqwest::Client::new();
        let response = get_response(&client, url, Some(request), &config.identity).await?;

        println!("Response: {}", response.text().await?);

        Ok(())
    }
}
