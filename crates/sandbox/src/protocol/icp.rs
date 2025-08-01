use core::time::Duration;
use std::net::TcpStream;

use candid::Principal;
use eyre::{bail, OptionExt, Result as EyreResult};
use ic_agent::identity::AnonymousIdentity;
use ic_agent::Agent;
use serde::{Deserialize, Serialize};
use url::Url;

/// Configuration for Internet Computer Protocol (ICP) sandbox environment
/// Contains necessary parameters for connecting to and interacting with ICP network
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IcpProtocolConfig {
    /// Canister ID of the deployed Context Config contract
    pub context_config_contract_id: String,
    /// URL of the ICP network endpoint (e.g., local replica)
    pub rpc_url: String,
    /// Principal ID used for transactions
    pub account_id: String,
    /// Public key for identity verification
    pub public_key: String,
    /// Private key for signing transactions
    pub secret_key: String,
}

impl Default for IcpProtocolConfig {
    fn default() -> Self {
        Self {
            context_config_contract_id: "bkyz2-fmaaa-aaaaa-qaaaq-cai".to_string(),
            rpc_url: "http://127.0.0.1:4943".to_string(),
            account_id: "fph2z-lxdui-xq3o6-6kuqy-rgkwi-hq7us-gkwlq-gxfgs-irrcq-hnm4e-6qe"
                .to_string(),
            public_key: "e3a22f0dbbde552188995641e1fa48cab2e06b94d24462281dace13d02".to_string(),
            secret_key: "c9a8e56920efd1c7b6694dce6ce871b661ae3922d5045d4a9f04e131eaa34164"
                .to_string(),
        }
    }
}

/// Represents the ICP sandbox environment for testing
/// Handles canister interactions and state verification
#[derive(Debug, Clone)]
pub struct IcpSandboxEnvironment {
    config: IcpProtocolConfig,
}

impl IcpSandboxEnvironment {
    /// Initialize a new ICP sandbox environment
    ///
    /// # Arguments
    /// * `config` - Configuration parameters for the ICP environment
    ///
    /// # Returns
    /// * `EyreResult<Self>` - New instance or error if connection fails
    ///
    /// # Errors
    /// * If RPC URL is invalid
    /// * If connection to RPC endpoint fails
    pub fn init(config: IcpProtocolConfig) -> EyreResult<Self> {
        // Parse and validate RPC URL
        let rpc_url = Url::parse(&config.rpc_url)?;
        let rpc_host = rpc_url
            .host_str()
            .ok_or_eyre("failed to get icp rpc host from config")?;
        let rpc_port = rpc_url
            .port()
            .ok_or_eyre("failed to get icp rpc port from config")?;

        // Test connection to RPC endpoint with timeout
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

    /// Generate node configuration arguments for ICP protocol
    ///
    /// # Returns
    /// * `Vec<String>` - List of configuration arguments for the node, including:
    ///   - Protocol and network settings
    ///   - Contract ID configuration
    ///   - Signer configuration with RPC URL and credentials
    pub fn node_args(&self) -> Vec<String> {
        vec![
            // Protocol and network configuration
            format!("context.config.icp.network=\"{}\"", "local"),
            format!(
                "context.config.icp.contract_id=\"{}\"",
                self.config.context_config_contract_id
            ),
            // Signer configuration
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

    /// Verify the state of an external canister by calling a specified method
    ///
    /// # Arguments
    /// * `contract_id` - Principal ID of the canister to verify
    /// * `method_name` - Name of the method to call
    /// * `_args_json` - Arguments to pass to the method (currently unused)
    ///
    /// # Returns
    /// * `EyreResult<Option<String>>` - Result of the canister call or error
    ///
    /// # Errors
    /// * If canister ID is invalid
    /// * If agent creation fails
    /// * If root key fetch fails
    /// * If query execution fails
    /// * If response decoding fails
    pub async fn verify_external_contract_state(
        &self,
        contract_id: &str,
        method_name: &str,
        _args_json: &[String],
    ) -> EyreResult<Option<String>> {
        // Parse the canister ID from text representation
        let canister_id = Principal::from_text(contract_id)
            .map_err(|e| eyre::eyre!("Invalid canister ID '{}': {}", contract_id, e))?;

        // Create an agent with anonymous identity for querying
        let agent = Agent::builder()
            .with_url(&self.config.rpc_url)
            .with_identity(AnonymousIdentity)
            .build()
            .map_err(|e| eyre::eyre!("Failed to create agent: {}", e))?;

        // Fetch the root key (required for local development)
        agent
            .fetch_root_key()
            .await
            .map_err(|e| eyre::eyre!("Failed to fetch root key: {}", e))?;

        // Encode empty arguments as Candid format
        let arg =
            candid::encode_one(()).map_err(|e| eyre::eyre!("Failed to encode argument: {}", e))?;

        // Execute the query against the canister
        let response = agent
            .query(&canister_id, method_name)
            .with_arg(arg)
            .call()
            .await
            .map_err(|e| eyre::eyre!("Query failed: {}", e))?;

        // Decode response as Vec<Vec<u8>> (expected format for get_calls)
        match candid::decode_one::<Vec<Vec<u8>>>(&response) {
            Ok(calls) => {
                // Convert binary data to strings and join with newlines
                let result = calls
                    .iter()
                    .map(|call| String::from_utf8_lossy(call).to_string())
                    .collect::<Vec<String>>()
                    .join("\n");

                if !result.is_empty() {
                    Ok(Some(result))
                } else {
                    Ok(None)
                }
            }
            Err(e) => {
                bail!("Failed to decode response: {}", e);
            }
        }
    }
}
