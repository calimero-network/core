use core::time::Duration;
use std::net::TcpStream;

use alloy::eips::BlockId;
use alloy::network::EthereumWallet;
use alloy::primitives::{keccak256, Address, Bytes};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolValue;
use alloy::transports::http::reqwest::Url as AlloyUrl;
use eyre::{bail, OptionExt, Result as EyreResult};
use serde::{Deserialize, Serialize};
use url::Url;

/// Configuration for Ethereum protocol sandbox environment
/// Contains necessary parameters for connecting to and interacting with Ethereum network
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EthereumProtocolConfig {
    /// Address of the deployed Context Config contract
    pub context_config_contract_id: String,
    /// URL of the Ethereum RPC endpoint
    pub rpc_url: String,
    /// Ethereum account address used for transactions
    pub account_id: String,
    /// Private key for signing transactions
    pub secret_key: String,
}

impl Default for EthereumProtocolConfig {
    fn default() -> Self {
        Self {
            context_config_contract_id: "0x5FbDB2315678afecb367f032d93F642f64180aa3".to_string(),
            rpc_url: "http://127.0.0.1:8545".to_string(),
            account_id: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string(),
            secret_key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
                .to_string(),
        }
    }
}

/// Represents the Ethereum sandbox environment for testing
/// Handles contract interactions and state verification
#[derive(Debug, Clone)]
pub struct EthereumSandboxEnvironment {
    config: EthereumProtocolConfig,
}

impl EthereumSandboxEnvironment {
    /// Initialize a new Ethereum sandbox environment
    ///
    /// # Arguments
    /// * `config` - Configuration parameters for the Ethereum environment
    ///
    /// # Returns
    /// * `EyreResult<Self>` - New instance or error if connection fails
    pub fn init(config: EthereumProtocolConfig) -> EyreResult<Self> {
        // Parse and validate RPC URL
        let rpc_url = Url::parse(&config.rpc_url)?;
        let rpc_host = rpc_url
            .host_str()
            .ok_or_eyre("failed to get ethereum rpc host from config")?;
        let rpc_port = rpc_url
            .port()
            .ok_or_eyre("failed to get ethereum rpc port from config")?;

        // Test connection to RPC endpoint
        if let Err(err) = TcpStream::connect_timeout(
            &format!("{rpc_host}:{rpc_port}").parse()?,
            Duration::from_secs(3),
        ) {
            bail!(
                "Failed to connect to ethereum rpc url '{}': {}",
                &config.rpc_url,
                err
            );
        }

        Ok(Self { config })
    }

    /// Generate node configuration arguments for Ethereum protocol
    ///
    /// # Returns
    /// * `Vec<String>` - List of configuration arguments for the node
    pub fn node_args(&self) -> Vec<String> {
        vec![
            // Protocol and network configuration
            format!("context.config.ethereum.network=\"{}\"", "sepolia"),
            format!(
                "context.config.ethereum.contract_id=\"{}\"",
                self.config.context_config_contract_id
            ),
            // Signer configuration
            format!("context.config.ethereum.signer=\"{}\"", "self"),
            format!(
                "context.config.signer.self.ethereum.sepolia.rpc_url=\"{}\"",
                self.config.rpc_url
            ),
            format!(
                "context.config.signer.self.ethereum.sepolia.account_id=\"{}\"",
                self.config.account_id
            ),
            format!(
                "context.config.signer.self.ethereum.sepolia.secret_key=\"{}\"",
                self.config.secret_key
            ),
        ]
    }

    /// Verify the state of an external contract by calling a specified method
    ///
    /// # Arguments
    /// * `contract_id` - Address of the contract to verify
    /// * `method_name` - Name of the method to call
    /// * `args` - Arguments to pass to the method
    ///
    /// # Returns
    /// * `EyreResult<Option<String>>` - Result of the contract call or error
    pub async fn verify_external_contract_state(
        &self,
        contract_id: &str,
        method_name: &str,
        args: &Vec<String>,
    ) -> EyreResult<Option<String>> {
        // Set up RPC connection
        let rpc_url = AlloyUrl::parse(&self.config.rpc_url)?;
        let address = contract_id.parse::<Address>()?;

        // Prepare method call data
        let method_selector = &keccak256(method_name.as_bytes())[..4];
        let encoded_args = match args.len() {
            1 => SolValue::abi_encode(&args[0].as_str()),
            _ => bail!("Unsupported number of arguments: {}", args.len()),
        };
        let call_data = [method_selector, &encoded_args].concat();

        // Set up wallet and provider
        let private_key = PrivateKeySigner::random();
        let wallet = EthereumWallet::from(private_key);
        let provider = ProviderBuilder::new()
            .wallet(wallet)
            .on_http(rpc_url)
            .erased();

        // Prepare and execute the call
        let request = TransactionRequest::default()
            .to(address)
            .input(Bytes::from(call_data).into());
        let result = provider.call(&request).block(BlockId::latest()).await?;

        // Decode and return the result
        let output: String = SolValue::abi_decode(&result, false)?;
        Ok(Some(output))
    }
}
