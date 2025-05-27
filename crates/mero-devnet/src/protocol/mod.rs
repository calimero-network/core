use eyre::Result;

pub mod ethereum;
pub mod icp;
pub mod near;
pub mod stellar;

#[derive(Debug, Clone)]
pub enum ProtocolSandboxEnvironment {
    Near(near::NearSandboxEnvironment),
    Icp(icp::IcpSandboxEnvironment),
    Stellar(stellar::StellarSandboxEnvironment),
    Ethereum(ethereum::EthereumSandboxEnvironment),
}

impl ProtocolSandboxEnvironment {
    pub async fn node_args(&self, node_name: &str) -> Result<Vec<String>> {
        match self {
            Self::Near(env) => env.node_args(node_name).await,
            Self::Icp(env) => Ok(env.node_args()),
            Self::Stellar(env) => Ok(env.node_args()),
            Self::Ethereum(env) => Ok(env.node_args()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Near(_) => "near",
            Self::Icp(_) => "icp",
            Self::Stellar(_) => "stellar",
            Self::Ethereum(_) => "ethereum",
        }
    }

    pub async fn verify_external_contract_state(
        &self,
        contract_id: &str,
        method_name: &str,
        args: &Vec<String>,
    ) -> Result<Option<String>> {
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
        }
    }
}
