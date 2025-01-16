use camino::Utf8PathBuf;
use eyre::Result as EyreResult;
use near_workspaces::network::Sandbox;
use near_workspaces::types::NearToken;
use near_workspaces::{Account, Contract, Worker};
use serde::{Deserialize, Serialize};
use tokio::fs::read;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NearProtocolConfig {
    pub context_config_contract: Utf8PathBuf,
    pub proxy_lib_contract: Utf8PathBuf,
}

pub struct NearSandboxEnvironment {
    pub worker: Worker<Sandbox>,
    pub root_account: Account,
    pub contract: Contract,
}

impl NearSandboxEnvironment {
    pub async fn init(config: NearProtocolConfig) -> EyreResult<Self> {
        let worker = near_workspaces::sandbox().await?;

        let wasm = read(&config.context_config_contract).await?;
        let context_config_contract = worker.dev_deploy(&wasm).await?;

        let proxy_lib_contract = read(&config.proxy_lib_contract).await?;
        drop(
            context_config_contract
                .call("set_proxy_code")
                .args(proxy_lib_contract)
                .max_gas()
                .transact()
                .await?
                .into_result()?,
        );

        let root_account = worker.root_account()?;

        Ok(Self {
            worker,
            root_account,
            contract: context_config_contract,
        })
    }

    pub async fn node_args(&self, node_name: &str) -> EyreResult<Vec<String>> {
        let near_account = self
            .root_account
            .create_subaccount(node_name)
            .initial_balance(NearToken::from_near(30))
            .transact()
            .await?
            .into_result()?;
        let near_secret_key = near_account.secret_key();

        Ok(vec![
            format!(
                "context.config.near.contract_id=\"{}\"",
                self.contract.as_account().id()
            ),
            format!("context.config.near.signer=\"{}\"", "self"),
            format!(
                "context.config.signer.self.near.testnet.rpc_url=\"{}\"",
                self.worker.rpc_addr()
            ),
            format!(
                "context.config.signer.self.near.testnet.account_id=\"{}\"",
                near_account.id()
            ),
            format!(
                "context.config.signer.self.near.testnet.public_key=\"{}\"",
                near_secret_key.public_key()
            ),
            format!(
                "context.config.signer.self.near.testnet.secret_key=\"{}\"",
                near_secret_key
            ),
        ])
    }
}
