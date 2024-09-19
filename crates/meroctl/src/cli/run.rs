use calimero_blobstore::config::BlobStoreConfig;
use calimero_network::config::NetworkConfig;
use calimero_node::{start, NodeConfig};
use calimero_node_primitives::NodeType as PrimitiveNodeType;
use calimero_server::config::ServerConfig;
use calimero_store::config::StoreConfig;
use clap::{Parser, ValueEnum};
use eyre::{bail, Result as EyreResult};

use crate::cli::RootArgs;
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

impl From<NodeType> for PrimitiveNodeType {
    fn from(value: NodeType) -> Self {
        match value {
            NodeType::Peer => Self::Peer,
            NodeType::Coordinator => Self::Coordinator,
        }
    }
}

impl RunCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let config = ConfigFile::load(&path)?;

        start(NodeConfig::new(
            path.clone(),
            config.identity.clone(),
            self.node_type.into(),
            NetworkConfig::new(
                config.identity.clone(),
                self.node_type.into(),
                config.network.swarm,
                config.network.bootstrap,
                config.network.discovery,
                config.network.catchup,
            ),
            StoreConfig::new(path.join(config.datastore.path)),
            BlobStoreConfig::new(path.join(config.blobstore.path)),
            config.context,
            ServerConfig::new(
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
