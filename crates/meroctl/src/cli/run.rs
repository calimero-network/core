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

#[derive(Copy, Clone, Debug, Default, ValueEnum)]
pub enum NodeType {
    #[default]
    Peer,
    Coordinator,
}

impl From<NodeType> for calimero_node_primitives::NodeType {
    fn from(value: NodeType) -> Self {
        match value {
            NodeType::Peer => calimero_node_primitives::NodeType::Peer,
            NodeType::Coordinator => calimero_node_primitives::NodeType::Coordinator,
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

        calimero_node::start(calimero_node::NodeConfig {
            home: path.clone(),
            node_type: self.node_type.into(),
            identity: config.identity.clone(),
            store: calimero_store::config::StoreConfig {
                path: path.join(config.store.path),
            },
            application: calimero_context::config::ApplicationConfig {
                dir: path.join(config.application.path),
                cathup: calimero_context::config::CatchupConfig {
                    batch_size: config.application.cathup.batch_size,
                },
            },
            network: calimero_network::config::NetworkConfig {
                identity: config.identity.clone(),
                node_type: self.node_type.into(),
                swarm: config.network.swarm,
                bootstrap: config.network.bootstrap,
                discovery: config.network.discovery,
            },
            server: calimero_server::config::ServerConfig {
                listen: config.network.server.listen,
                identity: config.identity,
                admin: config.network.server.admin,
                jsonrpc: config.network.server.jsonrpc,
                websocket: config.network.server.websocket,
            },
        })
        .await
    }
}
