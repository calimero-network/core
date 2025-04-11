use std::net::TcpStream;
use std::str::FromStr;
use std::time::Duration;

use eyre::{bail, OptionExt, Result as EyreResult};
use hex::encode;
use serde::{Deserialize, Serialize};
use url::Url;
use zksync_web3_rs::middleware::Middleware;
use zksync_web3_rs::providers::{Http, Provider};
use zksync_web3_rs::signers::{LocalWallet, Signer};
use zksync_web3_rs::types::{
    Address, Bytes, Eip1559TransactionRequest, NameOrAddress, TransactionRequest, H256, U256,
};
use zksync_web3_rs::utils::keccak256;

use crate::protocol::SandboxEnvironment;

/// Configuration for Zksync protocol sandbox environment
/// Contains necessary parameters for connecting to and interacting with Zksync network
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZksyncProtocolConfig {
    /// Address of the deployed Context Config contract
    pub context_config_contract_id: String,
    /// URL of the Zksync RPC endpoint
    pub rpc_url: String,
    /// Zksync account address used for transactions
    pub account_id: String,
    /// Private key for signing transactions
    pub secret_key: String,
}

/// Represents the Zksync sandbox environment for testing
/// Handles contract interactions and state verification
#[derive(Debug, Clone)]
pub struct ZksyncSandboxEnvironment {
    config: ZksyncProtocolConfig,
}

impl ZksyncSandboxEnvironment {
    /// Initialize a new Zksync sandbox environment
    ///
    /// # Arguments
    /// * `config` - Configuration parameters for the Zksync environment
    ///
    /// # Returns
    /// * `EyreResult<Self>` - New instance or error if connection fails
    pub fn init(config: ZksyncProtocolConfig) -> EyreResult<Self> {
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

    /// Generate node configuration arguments for Zksync protocol
    ///
    /// # Arguments
    /// * `node_name` - Name of the node to generate arguments for
    ///
    /// # Returns
    /// * `EyreResult<Vec<String>>` - List of configuration arguments for the node
    pub async fn node_args(&self) -> EyreResult<Vec<String>> {
        Ok(vec![
            // Protocol and network configuration
            format!("context.config.zksync.protocol=\"{}\"", "zksync"),
            format!("context.config.zksync.network=\"{}\"", "local"),
            format!(
                "context.config.zksync.contract_id=\"{}\"",
                self.config.context_config_contract_id
            ),
            // Signer configuration
            format!("context.config.zksync.signer=\"{}\"", "self"),
            format!(
                "context.config.signer.self.zksync.local.rpc_url=\"{}\"",
                self.config.rpc_url
            ),
            format!(
                "context.config.signer.self.zksync.local.account_id=\"{}\"",
                self.config.account_id
            ),
            format!(
                "context.config.signer.self.zksync.local.secret_key=\"{}\"",
                self.config.secret_key
            ),
        ])
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
        _args: &Vec<String>,
    ) -> EyreResult<Option<String>> {
        let http = Http::new(Url::parse(&self.config.rpc_url)?);
        let provider = Provider::new(http);
        let signer = LocalWallet::from_str(&self.config.secret_key)?;

        let address = Address::from_str(contract_id)?;
        let data = keccak256(method_name.as_bytes());
        let result = provider
            .call(
                &Eip1559TransactionRequest {
                    from: Some(signer.address()),
                    to: Some(NameOrAddress::Address(address)),
                    data: Some(Bytes::from(data.to_vec())),
                    ..Default::default()
                }
                .into(),
                None,
            )
            .await?;

        Ok(Some(format!("0x{}", encode(result))))
    }

    #[allow(
        dead_code,
        reason = "Method kept for future contract deployment functionality"
    )]
    pub async fn deploy_contract(&self, bytecode: &[u8]) -> EyreResult<String> {
        let http = Http::new(Url::parse(&self.config.rpc_url)?);
        let provider = Provider::new(http);
        let signer = LocalWallet::from_str(&self.config.secret_key)?;

        let nonce = provider
            .get_transaction_count(signer.address(), None)
            .await?;
        let gas_price = provider.get_gas_price().await?;
        let gas_limit = U256::from(3000000);

        let tx = TransactionRequest {
            from: Some(signer.address()),
            to: None,
            gas: Some(gas_limit),
            gas_price: Some(gas_price),
            value: None,
            nonce: Some(nonce),
            data: Some(Bytes::from(bytecode.to_vec())),
            ..Default::default()
        };

        let signed_tx = signer.sign_transaction(&tx.into()).await?;
        let tx_hash = H256::from_slice(
            &provider
                .send_raw_transaction(Bytes::from(signed_tx.to_vec()))
                .await?
                .0,
        );
        let receipt = provider.get_transaction_receipt(tx_hash).await?.unwrap();

        Ok(format!(
            "0x{}",
            encode(receipt.contract_address.unwrap().as_bytes())
        ))
    }
}

impl SandboxEnvironment for ZksyncSandboxEnvironment {
    fn node_args(&self) -> Vec<String> {
        // Block on the async call
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { self.node_args().await.unwrap_or_default() })
    }

    async fn verify_external_contract_state(
        &self,
        contract_id: &str,
        method_name: &str,
        args: &Vec<String>,
    ) -> EyreResult<Option<String>> {
        self.verify_external_contract_state(contract_id, method_name, args).await
    }
}
