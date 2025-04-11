use ethereum::EthereumSandboxEnvironment;
use eyre::Result as EyreResult;
use icp::IcpSandboxEnvironment;
use near::NearSandboxEnvironment;
use stellar::StellarSandboxEnvironment;
use zksync::ZksyncSandboxEnvironment;

pub mod ethereum;
pub mod icp;
pub mod near;
pub mod stellar;
pub mod zksync;

/// Trait defining the interface for protocol sandbox environments
pub trait SandboxEnvironment {
    /// Generate node configuration arguments for the protocol
    #[allow(dead_code)]
    fn node_args(&self) -> Vec<String>;

    /// Verify the state of an external contract
    #[allow(dead_code)]
    async fn verify_external_contract_state(
        &self,
        contract_id: &str,
        method_name: &str,
        args: &Vec<String>,
    ) -> EyreResult<Option<String>>;
}

#[derive(Debug, Clone)]
pub enum ProtocolSandboxEnvironment {
    Near(NearSandboxEnvironment),
    Icp(IcpSandboxEnvironment),
    Stellar(StellarSandboxEnvironment),
    Ethereum(EthereumSandboxEnvironment),
    Zksync(ZksyncSandboxEnvironment),
}

impl ProtocolSandboxEnvironment {
    pub async fn node_args(&self, node_name: &str) -> EyreResult<Vec<String>> {
        match self {
            Self::Near(env) => env.node_args(node_name).await,
            Self::Icp(env) => Ok(env.node_args()),
            Self::Stellar(env) => Ok(env.node_args()),
            Self::Ethereum(env) => Ok(env.node_args()),
            Self::Zksync(env) => env.node_args().await,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Near(_) => "near",
            Self::Icp(_) => "icp",
            Self::Stellar(_) => "stellar",
            Self::Ethereum(_) => "ethereum",
            Self::Zksync(_) => "zksync",
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
            Self::Zksync(env) => {
                env.verify_external_contract_state(contract_id, method_name, args)
                    .await
            }
        }
    }
}
