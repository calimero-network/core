use std::fs;

use clap::Parser;
use eyre::WrapErr;
use libp2p::identity;
use tracing::{info, warn};

use crate::cli;
use crate::config::{ConfigFile, ConfigImpl, InitFile};

/// Initialize node and it's identity
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

        fs::create_dir_all(&path)
            .wrap_err_with(|| format!("failed to create directory {:?}", &path))?;

        if InitFile::exists(&path) {
            match ConfigFile::load(&path) {
                Ok(config) => {
                    if self.force {
                        warn!(
                            "Overriding config.toml file for {}, keeping identity",
                            self.node_name
                        );
                        let config_new = InitFile {
                            identity: config.identity,
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
                Err(_err) => match InitFile::load(&path) {
                    Ok(_config) => {
                        if self.force {
                            eyre::bail!(
                                "Node {} is already initialized in {:?}\nCan not override node identity",
                                self.node_name,
                                path
                            );
                        } else {
                            eyre::bail!(
                                "Node {} is already initialized in {:?}",
                                self.node_name,
                                path
                            );
                        }
                    }
                    Err(err) => eyre::bail!("failed to load existing configuration: {}", err),
                },
            }
        }
        let identity = identity::Keypair::generate_ed25519();
        info!("Generated identity: {:?}", identity.public().to_peer_id());

        let config = InitFile {
            identity: identity.clone(),
        };

        config.save(&path)?;

        println!("{:?}", path);
        Ok(())
    }
}
