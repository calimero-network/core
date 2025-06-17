use std::net::IpAddr;

use camino::Utf8PathBuf;
use eyre::bail;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevnetConfig {
    /// Number of nodes to start in the network
    pub node_count: u32,

    /// List of protocols to enable (e.g. ["near", "ethereum"])
    pub protocols: Vec<String>,

    /// Host address for swarm connections
    pub swarm_host: String,

    /// Starting port for swarm connections (nodes will use sequential ports)
    pub start_swarm_port: u16,

    /// Host address for RPC servers  
    pub server_host: String,

    /// Starting port for RPC servers (nodes will use sequential ports)
    pub start_server_port: u16,

    /// Base directory for node data
    pub home_dir: Utf8PathBuf,

    /// Base name for nodes (will be appended with numbers)
    pub node_name: Utf8PathBuf,
}

impl DevnetConfig {
    pub fn new(
        node_count: u32,
        protocols: Vec<String>,
        swarm_host: String,
        start_swarm_port: u16,
        server_host: String,
        start_server_port: u16,
        home_dir: Utf8PathBuf,
        node_name: Utf8PathBuf,
    ) -> Self {
        Self {
            node_count,
            protocols,
            swarm_host,
            start_swarm_port,
            server_host,
            start_server_port,
            home_dir,
            node_name,
        }
    }

    pub fn validate(&self) -> eyre::Result<()> {
        if self.node_count == 0 {
            bail!("At least one node must be configured".to_string());
        }

        if self.protocols.is_empty() {
            bail!("At least one protocol must be specified".to_string());
        }

        if let Err(e) = self.swarm_host.parse::<IpAddr>() {
            bail!(format!("Invalid swarm host: {}", e));
        }

        if let Err(e) = self.server_host.parse::<IpAddr>() {
            bail!(format!("Invalid server host: {}", e));
        }

        if self.start_swarm_port == 0 {
            bail!("Swarm port must be greater than 0".to_string());
        }

        if self.start_server_port == 0 {
            bail!("Server port must be greater than 0".to_string());
        }

        Ok(())
    }
}
