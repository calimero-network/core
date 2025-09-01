use core::time::Duration;
use std::cell::RefCell;
use std::net::TcpStream;
use std::rc::Rc;
use std::sync::Arc;

use eyre::{bail, eyre, OptionExt, Result as EyreResult};
use serde::{Deserialize, Serialize};
use soroban_client::contract::{ContractBehavior, Contracts};
use soroban_client::network::{NetworkPassphrase, Networks};
use soroban_client::transaction::{TransactionBuilder, TransactionBuilderBehavior};
use soroban_client::xdr::{ScString, ScVal};
use soroban_client::{Options, Server};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StellarProtocolConfig {
    pub context_config_contract_id: String,
    pub rpc_url: String,
    pub public_key: String,
    pub secret_key: String,
}

#[derive(Debug, Clone)]
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

    pub async fn verify_external_contract_state(
        &self,
        contract_id: &str,
        method_name: &str,
        args: &Vec<String>,
    ) -> EyreResult<Option<String>> {
        let options: Options = Options {
            allow_http: true,
            timeout: 1_000,
            headers: Default::default(),
            friendbot_url: None,
        };

        let server = Arc::new(
            Server::new(self.config.rpc_url.as_str(), options).expect("Failed to create server"),
        );

        let account = server
            .get_account(self.config.public_key.as_str())
            .await
            .map_err(|e| eyre!("Failed to get account: {}", e))?;
        let account = Rc::new(RefCell::new(account));

        let contract =
            Contracts::new(contract_id).map_err(|_| eyre!("Failed to create contract"))?;

        // Build ScVal args from plain strings (only supports one string arg for now)
        let scval_args: Vec<ScVal> = match &args[..] {
            [v1] => vec![ScVal::String(ScString(v1.clone().try_into()?))],
            _ => bail!("Unsupported number of arguments: {}", args.len()),
        };

        let transaction = TransactionBuilder::new(account, Networks::standalone(), None)
            .fee(10000u32)
            .add_operation(contract.call(method_name, Some(scval_args)))
            .set_timeout(15)
            .expect("Transaction timeout")
            .build();

        let simulation = server
            .simulate_transaction(&transaction, None)
            .await
            .map_err(|e| eyre!("simulation failed: {}", e))?;

        let (ret_val, _auth) = simulation
            .to_result()
            .ok_or_else(|| eyre!("no simulation result"))?;

        let result_str = match ret_val {
            ScVal::String(s) => Some(s.to_string()),
            ScVal::Symbol(s) => Some(s.to_string()),
            _ => None,
        };

        Ok(result_str)
    }
}
