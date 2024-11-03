use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub network_layout: NetworkLayout,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkLayout {
    pub node_count: u32,
    pub start_swarm_port: u32,
    pub start_server_port: u32,
}
