use std::net::IpAddr;

use calimero_sandbox::protocol::ethereum::EthereumProtocolConfig;
use calimero_sandbox::protocol::icp::IcpProtocolConfig;
use calimero_sandbox::protocol::near::NearProtocolConfig;
use calimero_sandbox::protocol::stellar::StellarProtocolConfig;
use serde::{Deserialize, Serialize};

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
    Stellar(StellarProtocolConfig),
    Ethereum(EthereumProtocolConfig),
}
