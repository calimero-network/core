use core::time::Duration;
use std::net::TcpStream;

use eyre::{bail, OptionExt, Result as EyreResult};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvmProtocolConfig {
    pub context_config_contract_id: String,
    pub rpc_url: String,
    pub account_id: String,
    pub secret_key: String,
}

pub struct EvmSandboxEnvironment {
    config: EvmProtocolConfig,
}

impl EvmSandboxEnvironment {
    pub fn init(config: EvmProtocolConfig) -> EyreResult<Self> {
        let rpc_url = Url::parse(&config.rpc_url)?;
        let rpc_host = rpc_url
            .host_str()
            .ok_or_eyre("failed to get evm rpc host from config")?;
        let rpc_port = rpc_url
            .port()
            .ok_or_eyre("failed to get evm rpc port from config")?;

        if let Err(err) = TcpStream::connect_timeout(
            &format!("{rpc_host}:{rpc_port}").parse()?,
            Duration::from_secs(3),
        ) {
            bail!(
                "Failed to connect to evm rpc url '{}': {}",
                &config.rpc_url,
                err
            );
        }

        Ok(Self { config })
    }

    pub fn node_args(&self) -> Vec<String> {
        println!("config: {:?}", self.config);
        vec![
            format!("context.config.evm.protocol=\"{}\"", "evm"),
            format!("context.config.evm.network=\"{}\"", "sepolia"),
            format!(
                "context.config.evm.contract_id=\"{}\"",
                self.config.context_config_contract_id
            ),
            format!("context.config.evm.signer=\"{}\"", "self"),
            format!(
                "context.config.signer.self.evm.sepolia.rpc_url=\"{}\"",
                self.config.rpc_url
            ),
            format!(
                "context.config.signer.self.evm.sepolia.account_id=\"{}\"",
                self.config.account_id
            ),
            format!(
                "context.config.signer.self.evm.sepolia.secret_key=\"{}\"",
                self.config.secret_key
            ),
        ]
    }
}
