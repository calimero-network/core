use eyre::{Result as EyreResult, OptionExt, bail};
use core::time::Duration;
use std::net::TcpStream;
use serde::{Deserialize, Serialize};
use url::Url;
use zksync_web3_rs::types::{H160, U256, U64, NameOrAddress, TransactionRequest, BlockId, H256, BlockNumber};
use zksync_web3_rs::types::transaction::eip2718::TypedTransaction;
use zksync_web3_rs::providers::Middleware;
use zksync_web3_rs::utils;
use ethabi::{encode, decode, Token, ParamType};

/// Configuration for ZkSync protocol sandbox environment
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZkSyncProtocolConfig {
    /// Address of the deployed Context Config contract
    pub context_config_contract_id: String,
    /// URL of the ZkSync RPC endpoint
    pub rpc_url: String,
    /// Account address used for transactions
    pub account_id: String,
    /// Private key for signing transactions
    pub secret_key: String,
}

/// Represents the ZkSync sandbox environment for testing
#[derive(Debug, Clone)]
pub struct ZkSyncSandboxEnvironment {
    config: ZkSyncProtocolConfig,
}

impl ZkSyncSandboxEnvironment {
    /// Initialize a new ZkSync sandbox environment
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

    /// Generate node configuration arguments for ZkSync protocol
    pub async fn node_args(&self, _node_name: &str) -> EyreResult<Vec<String>> {
        Ok(vec![
            // Protocol and network configuration
            format!("context.config.zksync.protocol=\"{}\"", "zksync"),
            format!("context.config.zksync.network=\"{}\"", "testnet"),
            format!(
                "context.config.zksync.contract_id=\"{}\"",
                self.config.context_config_contract_id
            ),
            // Signer configuration
            format!("context.config.zksync.signer=\"{}\"", "self"),
            format!(
                "context.config.signer.self.zksync.testnet.rpc_url=\"{}\"",
                self.config.rpc_url
            ),
            format!(
                "context.config.signer.self.zksync.testnet.account_id=\"{}\"",
                self.config.account_id
            ),
            format!(
                "context.config.signer.self.zksync.testnet.secret_key=\"{}\"",
                self.config.secret_key
            ),
        ])
    }

    /// Verify the state of an external contract by calling a specified method
    pub async fn verify_external_contract_state(
        &self,
        contract_id: &str,
        method_name: &str,
        args: &Vec<String>,
    ) -> EyreResult<Option<String>> {
        // Create web3 provider
        let provider = zksync_web3_rs::providers::Provider::try_from(&self.config.rpc_url)?;
        
        // Parse contract address
        let contract_address = contract_id.parse::<H160>()?;
        let from_address = self.config.account_id.parse::<H160>()?;

        // Create the function call data
        let method_selector = &utils::keccak256(method_name.as_bytes())[..4];
        
        // Encode the arguments
        let mut encoded_args = Vec::new();
        for arg in args {
            // Encode each string argument
            let encoded = encode(&[Token::String(arg.clone())]);
            encoded_args.extend(encoded);
        }
        
        // Combine selector and encoded arguments
        let call_data = [method_selector, &encoded_args].concat();

        // Create the transaction request
        let tx_request = TransactionRequest {
            from: Some(from_address),
            to: Some(NameOrAddress::Address(contract_address)),
            gas: Some(U256::from(3000000)),
            gas_price: Some(U256::from(1000000000)),
            value: Some(U256::zero()),
            data: Some(call_data.into()),
            nonce: None,
            chain_id: None,
        };

        // Convert to TypedTransaction
        let typed_tx: TypedTransaction = tx_request.into();

        // For read operations, just call
        if method_name.starts_with("get") || method_name.starts_with("is") {
            let result = provider.call(&typed_tx, Some(BlockId::Number(BlockNumber::Number(U64::from(0))))).await?;
            let decoded = decode(&[ParamType::String], &result.0)?;
            Ok(Some(decoded[0].to_string()))
        } else {
            // For write operations, send transaction and wait for receipt
            let pending_tx = provider.send_transaction(typed_tx.clone(), None).await?;
            let tx_hash = pending_tx.tx_hash();
            
            let receipt = provider
                .get_transaction_receipt(tx_hash)
                .await?
                .ok_or_else(|| eyre::eyre!("Transaction receipt not found"))?;

            if receipt.status != Some(U64::from(1)) {
                bail!("Transaction failed");
            }

            // Get the return data from the transaction
            let result = provider.call(&typed_tx, Some(BlockId::Number(BlockNumber::Number(U64::from(0))))).await?;
            let decoded = decode(&[ParamType::String], &result.0)?;
            Ok(Some(decoded[0].to_string()))
        }
    }
}