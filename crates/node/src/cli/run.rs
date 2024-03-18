use calimero_node::config::ConfigFile;
use clap::{Parser, ValueEnum};

use crate::cli;

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

impl From<NodeType> for calimero_primitives::types::NodeType {
    fn from(value: NodeType) -> Self {
        match value {
            NodeType::Peer => calimero_primitives::types::NodeType::Peer,
            NodeType::Coordinator => calimero_primitives::types::NodeType::Coordinator,
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
            app_path: config.app.path,
            node_type: self.node_type.into(),
            identity: config.identity.clone(),
            store: calimero_store::config::StoreConfig {
                path: root_args.home.join(config.store.path),
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
                graphql: config.network.server.graphql,
                identity: config.identity,
            },
        })
        .await
    }
}
