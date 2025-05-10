use calimero_blobstore::config::BlobStoreConfig;
use calimero_config::ConfigFile;
use calimero_network::config::NetworkConfig;
use calimero_node::sync::SyncConfig;
use calimero_node::{start, NodeConfig};
use calimero_server::config::ServerConfig;
use calimero_store::config::StoreConfig;
use clap::Parser;
use eyre::{bail, Result as EyreResult};

use crate::cli::RootArgs;

/// Run a node
#[derive(Debug, Parser)]
pub struct RunCommand {
    #[arg(long, default_value_t)]
    pub auth: bool,
}

impl RunCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let config = ConfigFile::load(&path).await?;
        let mut server_config = ServerConfig::new(
            config.network.server.listen,
            config.identity.clone(),
            config.network.server.admin,
            config.network.server.jsonrpc,
            config.network.server.websocket,
        );

        if let Some(admin) = &mut server_config.admin {
            admin.auth_enabled = self.auth;
        }

        if let Some(jsonrpc) = &mut server_config.jsonrpc {
            jsonrpc.auth_enabled = self.auth;
        }

        start(NodeConfig::new(
            path.clone(),
            config.identity.clone(),
            NetworkConfig::new(
                config.identity.clone(),
                config.network.swarm,
                config.network.bootstrap,
                config.network.discovery,
            ),
            SyncConfig {
                timeout: config.sync.timeout,
                interval: config.sync.interval,
            },
            StoreConfig::new(path.join(config.datastore.path)),
            BlobStoreConfig::new(path.join(config.blobstore.path)),
            config.context,
            server_config,
        ))
        .await
    }
}
