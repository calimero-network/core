use core::time::Duration;
use std::net::TcpStream;

use eyre::{bail, OptionExt, Result as EyreResult};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StellarProtocolConfig {
    pub context_config_contract_id: String,
    pub rpc_url: String,
    pub public_key: String,
    pub secret_key: String,
}

pub struct StellarSandboxEnvironment {
    config: StellarProtocolConfig,
}

impl StellarSandboxEnvironment {
    pub fn init(config: StellarProtocolConfig) -> EyreResult<Self> {
        let rpc_url = Url::parse(&config.rpc_url)?;
        let rpc_host = rpc_url
            .host_str()
            .ok_or_eyre("failed to get stellar rpc host from config")?;
        let rpc_port = rpc_url
            .port()
            .ok_or_eyre("failed to get stellar rpc port from config")?;

        if let Err(err) = TcpStream::connect_timeout(
            &format!("{rpc_host}:{rpc_port}").parse()?,
            Duration::from_secs(3),
        ) {
            bail!(
                "Failed to connect to stellar rpc url '{}': {}",
                &config.rpc_url,
                err
            );
        }

        Ok(Self { config })
    }

    pub fn node_args(&self) -> Vec<String> {
        vec![
            format!("context.config.stellar.protocol=\"{}\"", "stellar"),
            format!("context.config.stellar.network=\"{}\"", "local"),
            format!(
                "context.config.stellar.contract_id=\"{}\"",
                self.config.context_config_contract_id
            ),
            format!("context.config.stellar.signer=\"{}\"", "self"),
            format!(
                "context.config.signer.self.stellar.local.rpc_url=\"{}\"",
                self.config.rpc_url
            ),
            format!(
                "context.config.signer.self.stellar.local.public_key=\"{}\"",
                self.config.public_key
            ),
            format!(
                "context.config.signer.self.stellar.local.secret_key=\"{}\"",
                self.config.secret_key
            ),
        ]
    }
}
