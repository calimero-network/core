use eyre::Result as EyreResult;
use icp::IcpSandboxEnvironment;
use near::NearSandboxEnvironment;
use stellar::StellarSandboxEnvironment;

pub mod icp;
pub mod near;
pub mod stellar;

pub enum ProtocolSandboxEnvironment {
    Near(NearSandboxEnvironment),
    Icp(IcpSandboxEnvironment),
    Stellar(StellarSandboxEnvironment),
}

impl ProtocolSandboxEnvironment {
    pub async fn node_args(&self, node_name: &str) -> EyreResult<Vec<String>> {
        match self {
            Self::Near(env) => env.node_args(node_name).await,
            Self::Icp(env) => Ok(env.node_args()),
            Self::Stellar(env) => Ok(env.node_args()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Near(_) => "near",
            Self::Icp(_) => "icp",
            Self::Stellar(_) => "stellar",
        }
    }
}
