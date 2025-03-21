use core::time::Duration;
use std::net::TcpStream;

use eyre::{bail, OptionExt, Result as EyreResult};
use serde::{Deserialize, Serialize};
use url::Url;
use reqwest::blocking::Client;
use serde_json::json;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IcpProtocolConfig {
    pub context_config_contract_id: String,
    pub rpc_url: String,
    pub account_id: String,
    pub public_key: String,
    pub secret_key: String,
}

#[derive(Debug, Clone)]
pub struct IcpSandboxEnvironment {
    config: IcpProtocolConfig,
}

impl IcpSandboxEnvironment {
    pub fn init(config: IcpProtocolConfig) -> EyreResult<Self> {
        let rpc_url = Url::parse(&config.rpc_url)?;
        let rpc_host = rpc_url
            .host_str()
            .ok_or_eyre("failed to get icp rpc host from config")?;
        let rpc_port = rpc_url
            .port()
            .ok_or_eyre("failed to get icp rpc port from config")?;

        if let Err(err) = TcpStream::connect_timeout(
            &format!("{rpc_host}:{rpc_port}").parse()?,
            Duration::from_secs(3),
        ) {
            bail!(
                "Failed to connect to icp rpc url '{}': {}",
                &config.rpc_url,
                err
            );
        }

        Ok(Self { config })
    }

    pub fn node_args(&self) -> Vec<String> {
        vec![
            format!("context.config.icp.protocol=\"{}\"", "icp"),
            format!("context.config.icp.network=\"{}\"", "local"),
            format!(
                "context.config.icp.contract_id=\"{}\"",
                self.config.context_config_contract_id
            ),
            format!("context.config.icp.signer=\"{}\"", "self"),
            format!(
                "context.config.signer.self.icp.local.rpc_url=\"{}\"",
                self.config.rpc_url
            ),
            format!(
                "context.config.signer.self.icp.local.account_id=\"{}\"",
                self.config.account_id
            ),
            format!(
                "context.config.signer.self.icp.local.public_key=\"{}\"",
                self.config.public_key
            ),
            format!(
                "context.config.signer.self.icp.local.secret_key=\"{}\"",
                self.config.secret_key
            ),
        ]
    }

    pub fn check_external_contract_state(&self, contract_id: &str, key: &str) -> EyreResult<Option<String>> {
        let rpc_url = Url::parse(&self.config.rpc_url)?;
        let rpc_host = rpc_url
            .host_str()
            .ok_or_eyre("failed to get icp rpc host from config")?;
        let rpc_port = rpc_url
            .port()
            .ok_or_eyre("failed to get icp rpc port from config")?;

        let client = Client::new();
        let endpoint = format!("http://{}:{}", rpc_host, rpc_port);

        // Prepare the JSON-RPC request for querying the contract
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "call",
            "params": {
                "canister_id": contract_id,
                "method_name": "get_value",
                "arg": json!({ "key": key })
            }
        });

        // Make the HTTP POST request
        let response = client.post(&endpoint)
            .json(&request)
            .send()?
            .json::<serde_json::Value>()?;

        // Check for errors in the response
        if let Some(error) = response.get("error") {
            bail!("JSON-RPC error when querying contract state: {}", error);
        }

        // Extract the result
        let result = response
            .get("result")
            .and_then(|r| r.get("output"))
            .ok_or_eyre("Failed to parse output from response")?;

        // Check if the result is null/empty
        if result.is_null() || result.as_array().map_or(false, |a| a.is_empty()) {
            return Ok(None);
        }

        // Convert result to string
        let value = result
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| result.get(0).and_then(|v| v.as_str()).map(|s| s.to_string()));

        Ok(value)
    }
}
