use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub network: Network,
    pub merod: MerodConfig,
    pub near: Near,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Network {
    pub node_count: u32,
    pub start_swarm_port: u32,
    pub start_server_port: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MerodConfig {
    pub args: Box<[String]>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Near {
    pub context_config_contract: Utf8PathBuf,
    pub proxy_lib_contract: Utf8PathBuf,
}
