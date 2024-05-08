use calimero_node::config::ConfigFile;
use clap::{Parser, ValueEnum};

use crate::cli;

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
        if !ConfigFile::exists(&root_args.home) {
            eyre::bail!("chat node is not initialized in {:?}", root_args.home);
        }

        let config = ConfigFile::load(&root_args.home)?;

        calimero_node::start(calimero_node::NodeConfig {
            home: root_args.home.clone(),
            node_type: self.node_type.into(),
            identity: config.identity.clone(),
            store: calimero_store::config::StoreConfig {
                path: root_args.home.join(config.store.path),
            },
            application: calimero_application::config::ApplicationConfig {
                dir: root_args.home.join(config.application.path),
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
