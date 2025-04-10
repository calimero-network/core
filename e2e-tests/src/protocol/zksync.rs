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
            format!("context.config.zksync.network=\"{}\"", "mainnet"),
            format!(
                "context.config.zksync.contract_id=\"{}\"",
                self.config.context_config_contract_id
            ),
            // Signer configuration
            format!("context.config.zksync.signer=\"{}\"", "self"),
            format!(
                "context.config.signer.self.zksync.mainnet.rpc_url=\"{}\"",
                self.config.rpc_url
            ),
            format!(
                "context.config.signer.self.zksync.mainnet.account_id=\"{}\"",
                self.config.account_id
            ),
            format!(
                "context.config.signer.self.zksync.mainnet.secret_key=\"{}\"",
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
        let access_key: PrivateKeySigner = PrivateKeySigner::from_str(&self.config.secret_key)
            .wrap_err("failed to convert secret key to PrivateKeySigner")?;

        let wallet = EthereumWallet::from(access_key);

        let provider: DynProvider<EthereumNetwork> = ProviderBuilder::new()
            .wallet(wallet)
            .on_http(AlloyUrl::parse(&self.config.rpc_url)?)
            .erased();

        let address = contract_id
            .parse::<Address>()
            .wrap_err("failed to parse contract address")?;

        let method_selector = &keccak256(method_name.as_bytes())[..4];

        let mut call_data = Vec::with_capacity(4 + args.len() * 32);
        call_data.extend_from_slice(method_selector);

        for arg in args {
            let bytes = hex::decode(arg.strip_prefix("0x").unwrap_or(arg))
                .wrap_err("failed to decode argument")?;
            call_data.extend_from_slice(&bytes);
        }

        let request = TransactionRequest::default()
            .to(address)
            .input(Bytes::from(call_data).into());

        let bytes = provider
            .call(&request)
            .block(BlockId::latest())
            .await
            .wrap_err("failed to execute eth_call")?;

        Ok(Some(format!("0x{}", hex::encode(bytes))))
    }
}
