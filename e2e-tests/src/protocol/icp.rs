use core::time::Duration;
use std::net::TcpStream;

use candid::Principal;
use eyre::{bail, OptionExt, Result as EyreResult};
use ic_agent::identity::AnonymousIdentity;
use ic_agent::Agent;
use serde::{Deserialize, Serialize};
use url::Url;

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

    pub async fn verify_external_contract_state(
        &self,
        contract_id: &str,
        method_name: &str,
        _args_json: &[String],
    ) -> EyreResult<Option<String>> {
        // Parse the canister ID
        let canister_id = Principal::from_text(contract_id)
            .map_err(|e| eyre::eyre!("Invalid canister ID '{}': {}", contract_id, e))?;

        // Create an agent with anonymous identity
        let agent = Agent::builder()
            .with_url(&self.config.rpc_url)
            .with_identity(AnonymousIdentity)
            .build()
            .map_err(|e| eyre::eyre!("Failed to create agent: {}", e))?;

        // Fetch the root key (needed for local development)
        agent
            .fetch_root_key()
            .await
            .map_err(|e| eyre::eyre!("Failed to fetch root key: {}", e))?;

        // Simply encode the args_json value, empty or not
        let arg =
            candid::encode_one(()).map_err(|e| eyre::eyre!("Failed to encode argument: {}", e))?;

        // Query the canister
        let response = agent
            .query(&canister_id, method_name)
            .with_arg(arg)
            .call()
            .await
            .map_err(|e| eyre::eyre!("Query failed: {}", e))?;

        // Just decode as Vec<Vec<u8>> for get_calls
        match candid::decode_one::<Vec<Vec<u8>>>(&response) {
            Ok(calls) => {
                // Convert all calls to strings and return as a single string
                let result = calls
                    .iter()
                    .map(|call| String::from_utf8_lossy(call).to_string())
                    .collect::<Vec<String>>()
                    .join("\n");

                if !result.is_empty() {
                    return Ok(Some(result));
                } else {
                    return Ok(None);
                }
            }
            Err(e) => {
                bail!("Failed to decode response: {}", e);
            }
        }
    }
}
