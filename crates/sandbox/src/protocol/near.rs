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
    pub mock_external_contract: Utf8PathBuf,
}

impl Default for NearProtocolConfig {
    fn default() -> Self {
        Self {
            context_config_contract: "contracts/near/calimero_context_config_near.wasm".into(),
            proxy_lib_contract: "contracts/near/calimero_context_proxy_near.wasm".into(),
            mock_external_contract: "contracts/near/calimero_mock_external_near.wasm".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NearSandboxEnvironment {
    pub worker: Worker<Sandbox>,
    pub root_account: Account,
    pub contract: Contract,
    pub mock_external_contract: Contract,
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

        // Create a fixed-name account for the mock external contract
        let mock_external_account = root_account
            .create_subaccount("mock-external")
            .initial_balance(NearToken::from_near(30))
            .transact()
            .await?
            .into_result()?;

        // Deploy to the fixed-name account
        let mock_external_wasm = read(&config.mock_external_contract).await?;
        let mock_external_contract = mock_external_account.deploy(&mock_external_wasm).await?;

        let mock_external_contract = mock_external_contract.into_result()?;

        // Initialize the counter contract with new()
        mock_external_contract
            .call("new")
            .transact()
            .await?
            .into_result()?;

        Ok(Self {
            worker,
            root_account,
            contract: context_config_contract,
            mock_external_contract,
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

    pub async fn verify_external_contract_state(
        &self,
        _contract_id: &str,
        method_name: &str,
        args: &Vec<String>,
    ) -> EyreResult<Option<String>> {
        // Join all arguments into a single Vec<u8>
        let arguments = args
            .iter()
            .flat_map(|arg| arg.as_bytes())
            .copied()
            .collect::<Vec<u8>>();

        let result = self
            .mock_external_contract
            .view(method_name)
            .args(arguments)
            .await?;

        let result_value: u32 = result
            .json()
            .map_err(|e| eyre::eyre!("Failed to parse result as JSON: {}", e))?;

        Ok(Some(result_value.to_string()))
    }
}
