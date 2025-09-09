use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::protocol::ethereum::EthereumProtocolConfig;
use crate::protocol::icp::IcpProtocolConfig;
use crate::protocol::near::NearProtocolConfig;

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
    pub swarm_host: IpAddr,
    pub server_host: IpAddr,
    pub start_swarm_port: u16,
    pub start_server_port: u16,
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
    Ethereum(EthereumProtocolConfig),
}
