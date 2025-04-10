use core::time::Duration;
use std::net::TcpStream;
use std::str::FromStr;

use alloy::eips::BlockId;
use alloy::network::EthereumWallet;
use alloy::primitives::{keccak256, Address, Bytes};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolValue;
use alloy::transports::http::reqwest::Url as AlloyUrl;
use eyre::{bail, OptionExt, Result as EyreResult, WrapErr};
use hex;
use serde::{Deserialize, Serialize};
use url::Url;
use zksync_web3_rs::types::{BlockNumber, Eip712Meta};

use crate::protocol::SandboxEnvironment;

/// Configuration for zkSync protocol sandbox environment
/// Contains necessary parameters for connecting to and interacting with zkSync network
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZkSyncProtocolConfig {
    /// Address of the deployed Context Config contract
    pub context_config_contract_id: String,
    /// URL of the zkSync RPC endpoint
    pub rpc_url: String,
    /// zkSync account address used for transactions
    pub account_id: String,
    /// Private key for signing transactions
    pub secret_key: String,
}

/// Represents the zkSync sandbox environment for testing
/// Handles contract interactions and state verification
#[derive(Debug, Clone)]
pub struct ZkSyncSandboxEnvironment {
    config: ZkSyncProtocolConfig,
}

impl ZkSyncSandboxEnvironment {
    /// Initialize a new zkSync sandbox environment
    ///
    /// # Arguments
    /// * `config` - Configuration parameters for the zkSync environment
    ///
    /// # Returns
    /// * `EyreResult<Self>` - New instance or error if connection fails
    pub fn init(config: ZkSyncProtocolConfig) -> EyreResult<Self> {
        // Parse and validate RPC URL
        let rpc_url = Url::parse(&config.rpc_url)?;
        let rpc_host = rpc_url
            .host_str()
            .ok_or_eyre("failed to get zksync rpc host from config")?;
        let rpc_port = rpc_url
            .port()
            .ok_or_eyre("failed to get zksync rpc port from config")?;

        // Test connection to RPC endpoint
        if let Err(err) = TcpStream::connect_timeout(
            &format!("{rpc_host}:{rpc_port}").parse()?,
            Duration::from_secs(3),
        ) {
            bail!(
                "Failed to connect to zksync rpc url '{}': {}",
                &config.rpc_url,
                err
            );
        }

        Ok(Self { config })
    }

    /// Generate node configuration arguments for zkSync protocol
    ///
    /// # Returns
    /// * `Vec<String>` - List of configuration arguments for the node
    pub fn node_args(&self) -> Vec<String> {
        vec![
            // Protocol and network configuration
            format!("context.config.zksync.protocol=\"{}\"", "zksync"),
            format!("context.config.zksync.network=\"{}\"", "sepolia"),
            format!(
                "context.config.zksync.contract_id=\"{}\"",
                self.config.context_config_contract_id
            ),
            // Signer configuration
            format!("context.config.zksync.signer=\"{}\"", "self"),
            format!(
                "context.config.signer.self.zksync.sepolia.rpc_url=\"{}\"",
                self.config.rpc_url
            ),
            format!(
                "context.config.signer.self.zksync.sepolia.account_id=\"{}\"",
                self.config.account_id
            ),
            format!(
                "context.config.signer.self.zksync.sepolia.secret_key=\"{}\"",
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

impl SandboxEnvironment for ZkSyncSandboxEnvironment {
    fn node_args(&self) -> Vec<String> {
        self.node_args()
    }

    async fn verify_external_contract_state(
        &self,
        contract_id: &str,
        method_name: &str,
        args: &Vec<String>,
    ) -> EyreResult<Option<String>> {
        self.verify_external_contract_state(contract_id, method_name, args)
            .await
    }
} 