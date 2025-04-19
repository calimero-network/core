use ethereum::EthereumSandboxEnvironment;
use eyre::Result as EyreResult;
use icp::IcpSandboxEnvironment;
use near::NearSandboxEnvironment;
use stellar::StellarSandboxEnvironment;
use zksync::ZkSyncSandboxEnvironment;

pub mod ethereum;
pub mod icp;
pub mod near;
pub mod stellar;
pub mod zksync;

#[derive(Debug, Clone)]
pub enum ProtocolSandboxEnvironment {
    Near(NearSandboxEnvironment),
    Icp(IcpSandboxEnvironment),
    Stellar(StellarSandboxEnvironment),
    Ethereum(EthereumSandboxEnvironment),
    ZkSync(ZkSyncSandboxEnvironment),
}

impl ProtocolSandboxEnvironment {
    pub async fn node_args(&self, node_name: &str) -> EyreResult<Vec<String>> {
        match self {
            Self::Near(env) => env.node_args(node_name).await,
            Self::Icp(env) => Ok(env.node_args()),
            Self::Stellar(env) => Ok(env.node_args()),
            Self::Ethereum(env) => Ok(env.node_args()),
            Self::ZkSync(env) => env.node_args(node_name).await,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Near(_) => "near",
            Self::Icp(_) => "icp",
            Self::Stellar(_) => "stellar",
            Self::Ethereum(_) => "ethereum",
            Self::ZkSync(_) => "zksync",
        }
    }

    pub async fn verify_external_contract_state(
        &self,
        contract_id: &str,
        method_name: &str,
        args: &Vec<String>,
    ) -> EyreResult<Option<String>> {
        match self {
            Self::Near(env) => {
                env.verify_external_contract_state(contract_id, method_name, args)
                    .await
            }
            Self::Icp(env) => {
                env.verify_external_contract_state(contract_id, method_name, args)
                    .await
            }
            Self::Stellar(env) => {
                env.verify_external_contract_state(contract_id, method_name, args)
                    .await
            }
            Self::Ethereum(env) => {
                env.verify_external_contract_state(contract_id, method_name, args)
                    .await
            }
            Self::ZkSync(env) => {
                env.verify_external_contract_state(contract_id, method_name, args)
                    .await
            }
        }
    }
}
