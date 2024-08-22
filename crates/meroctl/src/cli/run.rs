use clap::{Parser, ValueEnum};

use crate::cli;
use crate::config_file::ConfigFile;

/// Run a node
#[derive(Debug, Parser)]
pub struct RunCommand {
    #[clap(long, value_name = "TYPE")]
    #[clap(value_enum, default_value_t)]
    pub node_type: NodeType,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum NodeType {
    #[default]
    Peer,
    Coordinator,
}

impl From<NodeType> for calimero_node_primitives::NodeType {
    fn from(value: NodeType) -> Self {
        match value {
            NodeType::Peer => Self::Peer,
            NodeType::Coordinator => Self::Coordinator,
        }
    }
}

impl RunCommand {
    pub async fn run(self, root_args: cli::RootArgs) -> eyre::Result<()> {
        let path = root_args.home.join(root_args.node_name);

        if !ConfigFile::exists(&path) {
            eyre::bail!("Node is not initialized in {:?}", path);
        }

        let config = ConfigFile::load(&path)?;

        calimero_node::start(calimero_node::NodeConfig::new(
            path.clone(),
            self.node_type.into(),
            config.identity.clone(),
            calimero_store::config::StoreConfig::new(path.join(config.store.path)),
            calimero_context::config::ApplicationConfig::new(path.join(config.application.path)),
            calimero_network::config::NetworkConfig::new(
                config.identity.clone(),
                self.node_type.into(),
                config.network.swarm,
                config.network.bootstrap,
                config.network.discovery,
                config.network.catchup,
            ),
            calimero_server::config::ServerConfig::new(
                config.network.server.listen,
                config.identity.clone(),
                config.network.server.admin,
                config.network.server.jsonrpc,
                config.network.server.websocket,
            ),
        ))
        .await
    }
}
