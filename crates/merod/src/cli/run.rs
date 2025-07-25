use std::collections::HashMap;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_config::ConfigFile;
use calimero_network_primitives::config::NetworkConfig;
use calimero_node::sync::SyncConfig;
use calimero_node::{start, NodeConfig};
use calimero_server::admin::service::AdminConfig;
use calimero_server::config::ServerConfig;
use calimero_store::config::StoreConfig;
use clap::Parser;
use eyre::{bail, Result as EyreResult};

use crate::cli::RootArgs;

/// Run a node
#[derive(Debug, Parser)]
pub struct RunCommand {
    /// Protocol configuration arguments in key=value format
    #[clap(long, value_parser = parse_key_val::<String, String>)]
    pub protocol_config: Vec<(String, String)>,

    /// Enable admin API
    #[clap(long)]
    pub admin: bool,
}

impl RunCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let config = ConfigFile::load(&path).await?;
        let server_config = ServerConfig::new(
            config.network.server.listen,
            config.identity.clone(),
            // Pass admin flag
            if self.admin {
                Some(AdminConfig::new(true))
            } else {
                None
            },
            config.network.server.jsonrpc,
            config.network.server.websocket,
        );

        // Convert protocol config args to HashMap
        let protocol_config: HashMap<String, String> = self.protocol_config.into_iter().collect();

        start(NodeConfig {
            home: path.clone(),
            identity: config.identity.clone(),
            network: NetworkConfig::new(
                config.identity.clone(),
                config.network.swarm,
                config.network.bootstrap,
                config.network.discovery,
            ),
            sync: SyncConfig {
                timeout: config.sync.timeout,
                interval: config.sync.interval,
                frequency: config.sync.frequency,
            },
            datastore: StoreConfig::new(path.join(config.datastore.path)),
            blobstore: BlobStoreConfig::new(path.join(config.blobstore.path)),
            context: config.context,
            server: server_config,
            protocol_config,
        })
        .await
    }
}

fn parse_key_val<T, U>(
    s: &str,
) -> Result<(T, U), Box<dyn std::error::Error + Send + Sync + 'static>>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
    U: std::str::FromStr,
    U::Err: std::error::Error + Send + Sync + 'static,
{
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{}`", s))?;
    Ok((s[..pos].parse()?, s[pos + 1..].parse()?))
}
