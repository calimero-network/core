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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EthereumProtocolConfig {
    pub context_config_contract_id: String,
    pub rpc_url: String,
    pub account_id: String,
    pub secret_key: String,
}

#[derive(Debug, Clone)]
pub struct EthereumSandboxEnvironment {
    config: EthereumProtocolConfig,
}

impl EthereumSandboxEnvironment {
    pub fn init(config: EthereumProtocolConfig) -> EyreResult<Self> {
        let rpc_url = Url::parse(&config.rpc_url)?;
        let rpc_host = rpc_url
            .host_str()
            .ok_or_eyre("failed to get ethereum rpc host from config")?;
        let rpc_port = rpc_url
            .port()
            .ok_or_eyre("failed to get ethereum rpc port from config")?;

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

    pub fn node_args(&self) -> Vec<String> {
        vec![
            format!("context.config.ethereum.protocol=\"{}\"", "ethereum"),
            format!("context.config.ethereum.network=\"{}\"", "sepolia"),
            format!(
                "context.config.ethereum.contract_id=\"{}\"",
                self.config.context_config_contract_id
            ),
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

    pub async fn verify_external_contract_state(
        &self,
        contract_id: &str,
        method_name: &str,
        args: &Vec<String>,
    ) -> EyreResult<Option<String>> {
        let rpc_url = AlloyUrl::parse(&self.config.rpc_url)?;
        let address = contract_id.parse::<Address>()?;
        let method_selector = &keccak256(method_name.as_bytes())[..4];

        let encoded_args = match args.len() {
            1 => SolValue::abi_encode(&args[0].as_str()),
            _ => bail!("Unsupported number of arguments: {}", args.len()),
        };

        let call_data = [method_selector, &encoded_args].concat();

        let private_key = PrivateKeySigner::random();
        let wallet = EthereumWallet::from(private_key);

        let provider = ProviderBuilder::new()
            .wallet(wallet)
            .on_http(rpc_url)
            .erased();

        let request = TransactionRequest::default()
            .to(address)
            .input(Bytes::from(call_data).into());

        let result = provider.call(&request).block(BlockId::latest()).await?;

        let output: String = SolValue::abi_decode(&result, false)?;

        Ok(Some(output.to_string()))
    }
}
