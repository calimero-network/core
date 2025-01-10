use eyre::Result as EyreResult;
use icp::IcpSandboxEnvironment;
use near::NearSandboxEnvironment;

pub mod icp;
pub mod near;

pub enum ProtocolSandboxEnvironment {
    Near(NearSandboxEnvironment),
    Icp(IcpSandboxEnvironment),
}

impl ProtocolSandboxEnvironment {
    pub async fn node_args(&self, node_name: &str) -> EyreResult<Vec<String>> {
        match self {
            ProtocolSandboxEnvironment::Near(env) => env.node_args(node_name).await,
            ProtocolSandboxEnvironment::Icp(env) => env.node_args(),
        }
    }

    pub fn name(&self) -> String {
        match self {
            ProtocolSandboxEnvironment::Near(_) => "near".to_string(),
            ProtocolSandboxEnvironment::Icp(_) => "icp".to_string(),
        }
    }
}
