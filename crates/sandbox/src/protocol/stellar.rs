use core::time::Duration;
use std::cell::RefCell;
use std::io::Cursor;
use std::net::TcpStream;
use std::rc::Rc;
use std::sync::Arc;

use base64::Engine;
use eyre::{bail, eyre, OptionExt, Result as EyreResult};
use serde::{Deserialize, Serialize};
use soroban_client::contract::{ContractBehavior, Contracts};
use soroban_client::error::Error;
use soroban_client::network::{NetworkPassphrase, Networks};
use soroban_client::server::{Options, Server};
use soroban_client::soroban_rpc::{RawSimulateHostFunctionResult, RawSimulateTransactionResponse};
use soroban_client::transaction::{ReadXdr, TransactionBuilder, TransactionBuilderBehavior};
use soroban_client::xdr::ScVal;
use soroban_sdk::xdr::{FromXdr, Limited, Limits, ToXdr};
use soroban_sdk::{Env, String as SorobanString};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StellarProtocolConfig {
    pub context_config_contract_id: String,
    pub rpc_url: String,
    pub public_key: String,
    pub secret_key: String,
}

impl Default for StellarProtocolConfig {
    fn default() -> Self {
        Self {
            context_config_contract_id: "CCIZDIWGDVCMO2PIR6VFW63GZNUAA47UYG4VUDL3XXN3DYJ2HWLIWIGD"
                .to_string(),
            rpc_url: "http://127.0.0.1:8000/soroban/rpc".to_string(),
            public_key: "GDIY6AQQ75WMD4W46EYB7O6UYMHOCGQHLAQGQTKHDX4J2DYQCHVCR4W4".to_string(),
            secret_key: "SC36BWNUOCZAO7DMEJNNKFV6BOTPJP7IG5PSHLUOLT6DZFRU3D3XGIXW".to_string(),
        }
    }
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
            allow_http: Some(true),
            timeout: Some(1000),
            headers: None,
        };

        let env = Env::default();

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

        // Match the args array to create appropriate tuples
        let scval_args = match &args[..] {
            [v1] => (SorobanString::from_str(&env, v1),),
            _ => bail!("Unsupported number of arguments: {}", args.len()),
        };

        let xdr = scval_args.to_xdr(&env);

        let vals: soroban_sdk::Vec<ScVal> =
            soroban_sdk::Vec::from_xdr(&env, &xdr).map_err(|_| eyre!("Failed to decode XDR"))?;

        let encoded_args = Some(vals.iter().collect::<Vec<_>>());

        let transaction = TransactionBuilder::new(account, Networks::standalone(), None)
            .fee(10000u32)
            .add_operation(contract.call(method_name, encoded_args))
            .set_timeout(15)
            .expect("Transaction timeout")
            .build();

        let result: Result<RawSimulateTransactionResponse, Error> =
            server.simulate_transaction(transaction, None).await;

        let xdr_results: Vec<RawSimulateHostFunctionResult> = result.unwrap().results.unwrap();

        let xdr_bytes = match xdr_results.first().and_then(|xdr| xdr.xdr.as_ref()) {
            Some(xdr_bytes) => base64::engine::general_purpose::STANDARD
                .decode(xdr_bytes)
                .map_err(|_| eyre!("Failed to decode XDR"))?,
            None => return Err(eyre!("No XDR results found")),
        };

        let cursor = Cursor::new(xdr_bytes);
        let mut limited = Limited::new(cursor, Limits::none());

        let sc_val =
            ScVal::read_xdr(&mut limited).map_err(|e| eyre::eyre!("Failed to read XDR: {}", e))?;

        let result_str = match sc_val {
            ScVal::String(s) => Some(s.to_string()),
            ScVal::Symbol(s) => Some(s.to_string()),
            ScVal::Bool(_)
            | ScVal::Void
            | ScVal::Error(_)
            | ScVal::U32(_)
            | ScVal::I32(_)
            | ScVal::U64(_)
            | ScVal::I64(_)
            | ScVal::Timepoint(_)
            | ScVal::Duration(_)
            | ScVal::U128(_)
            | ScVal::I128(_)
            | ScVal::U256(_)
            | ScVal::I256(_)
            | ScVal::Bytes(_)
            | ScVal::Vec(_)
            | ScVal::Map(_)
            | ScVal::Address(_)
            | ScVal::LedgerKeyContractInstance
            | ScVal::LedgerKeyNonce(_)
            | ScVal::ContractInstance(_) => None,
        };

        Ok(result_str)
    }
}
