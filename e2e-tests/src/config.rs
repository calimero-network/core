use serde::{Deserialize, Serialize};

use crate::protocol::icp::IcpProtocolConfig;
use crate::protocol::near::NearProtocolConfig;
use crate::protocol::stellar::StellarProtocolConfig;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub network: Network,
    pub merod: MerodConfig,
    pub protocol_sandboxes: Box<[ProtocolSandboxConfig]>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Network {
    pub node_count: u32,
    pub swarm_host: String,
    pub start_swarm_port: u32,
    pub start_server_port: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MerodConfig {
    pub args: Box<[String]>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "protocol", content = "config", rename_all = "camelCase")]
pub enum ProtocolSandboxConfig {
    Near(NearProtocolConfig),
    Icp(IcpProtocolConfig),
    Stellar(StellarProtocolConfig),
}
