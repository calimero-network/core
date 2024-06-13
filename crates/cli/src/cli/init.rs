use std::fs;

use calimero_node::config::ConfigFile;
use clap::Parser;
use eyre::WrapErr;
use libp2p::identity;
use tracing::{info, warn};

use crate::cli;

/// Initialize node configuration
#[derive(Debug, Parser)]

pub struct InitCommand {
    /// Name of node
    #[arg(short, long, value_name = "NAME")]
    pub node_name: camino::Utf8PathBuf,

    /// Force initialization even if the directory already exists
    #[clap(short, long)]
    pub force: bool,
}

impl InitCommand {
    pub fn run(self, root_args: cli::RootArgs) -> eyre::Result<()> {
        let path = root_args.home.join(&self.node_name);
        // tu dodati neki if il nes
        fs::create_dir_all(&path)
            .wrap_err_with(|| format!("failed to create directory {:?}", &path))?;

        if ConfigFile::exists(&path) {
            match ConfigFile::load(&path) {
                Ok(config) => {
                    if self.force {
                        warn!(
                            "Overriding config.toml file for {}, keeping identity",
                            self.node_name
                        );
                        let config_new = ConfigFile {
                            identity: config.identity,
                            network: None,
                            store: None,
                            application: None,
                        };
                        config_new.save(&path)?;
                        return Ok(());
                    } else {
                        eyre::bail!(
                            "Node {} is already initialized in {:?}",
                            self.node_name,
                            path
                        );
                    }
                }
                Err(err) => {
                    if !self.force {
                        eyre::bail!("failed to load existing configuration: {}", err);
                    }
                    warn!(
                        "Failed to load existing configuration, overwriting: {}",
                        err
                    );
                }
            }
        }
        let identity = identity::Keypair::generate_ed25519();
        info!("Generated identity: {:?}", identity.public().to_peer_id());

        let config = ConfigFile {
            identity: identity.clone(),
            network: None,
            store: None,
            application: None,
        };

        config.save(&path)?;

        println!("{:?}", path);
        Ok(())
    }
}
