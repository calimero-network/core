use color_eyre::eyre;

pub mod config;

pub struct NodeConfig {
    pub home: camino::Utf8PathBuf,
    pub node_type: calimero_primitives::types::NodeType,
    pub network: calimero_network::config::NetworkConfig,
}

pub async fn start(config: NodeConfig) -> eyre::Result<()> {
    // calimero_storage::init
    // calimero_network::init

    Ok(())
}
