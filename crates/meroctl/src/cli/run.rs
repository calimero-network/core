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

        start(NodeConfig {
            home: path.clone(),
            node_type: self.node_type.into(),
            identity: config.identity.clone(),
            network: NetworkConfig::new(
                config.identity.clone(),
                self.node_type.into(),
                config.network.swarm,
                config.network.bootstrap,
                config.network.discovery,
                config.network.catchup,
            ),
            datastore: StoreConfig::new(path.join(config.datastore.path)),
            blobstore: BlobStoreConfig::new(path.join(config.blobstore.path)),
            context: config.context,
            server: ServerConfig {
                listen: config.network.server.listen,
                identity: config.identity.clone(),
                admin: config.network.server.admin,
                jsonrpc: config.network.server.jsonrpc,
                websocket: config.network.server.websocket,
            },
        })
        .await
    }
}
