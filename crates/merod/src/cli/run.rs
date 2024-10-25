use calimero_blobstore::config::BlobStoreConfig;
use calimero_config::ConfigFile;
use calimero_network::config::NetworkConfig;
use calimero_node::{start, NodeConfig};
use calimero_server::config::ServerConfig;
use calimero_store::config::StoreConfig;
use clap::Parser;
use eyre::{bail, Result as EyreResult};

use crate::cli::RootArgs;

/// Run a node
#[derive(Debug, Parser)]
pub struct RunCommand;

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
            NetworkConfig::new(
                config.identity.clone(),
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
