use camino::Utf8PathBuf;
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
    pub swarm_host_env: String,
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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NearProtocolConfig {
    pub context_config_contract: Utf8PathBuf,
    pub proxy_lib_contract: Utf8PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IcpProtocolConfig {
    pub context_config_contract_id: String,
    pub rpc_url: String,
    pub account_id: String,
    pub public_key: String,
    pub secret_key: String,
}
