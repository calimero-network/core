use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_file as symlink;

use clap::Parser;
use eyre::WrapErr;
use sha2::{Digest, Sha256};
use tracing::info;

use crate::cli;
use crate::config::{ConfigFile, ConfigImpl};

/// Setup symlink to application in the node
#[derive(Debug, Parser)]
pub struct LinkCommand {
    /// Name of node
    #[arg(short, long, value_name = "NAME")]
    pub node_name: camino::Utf8PathBuf,

    /// Path to original file
    #[clap(short, long)]
    pub path: camino::Utf8PathBuf,

    /// Name of application
    #[clap(short, long)]
    pub app_name: camino::Utf8PathBuf,
}

impl LinkCommand {
    pub fn run(self, root_args: cli::RootArgs) -> eyre::Result<()> {
        let path_to_node = root_args.home.join(&self.node_name);
        if ConfigFile::exists(&path_to_node) {
            match ConfigFile::load(&path_to_node) {
                Ok(config) => {
                    let id = format!("{}:{}", self.node_name, self.app_name);
                    let mut hasher = Sha256::new();
                    hasher.update(id.as_bytes());
                    let hash_string = hex::encode(hasher.finalize());

                    let app_path = path_to_node.join(config.application.path).join(hash_string);

                    fs::create_dir_all(&app_path)
                        .wrap_err_with(|| format!("failed to create directory {:?}", &app_path))?;
                    info!("Linking original file to: {:?}", app_path);

                    symlink(self.path, app_path.join("binary.wasm"))?;
                    info!(
                        "Application {} linked to node {}\nPath to linked file at {}",
                        self.app_name,
                        self.node_name,
                        app_path.join("binary.wasm")
                    );
                    return Ok(());
                }
                Err(err) => {
                    eyre::bail!("failed to load existing configuration: {}", err);
                }
            }
        } else {
            eyre::bail!("You have to initialize the node first \nRun command node init -n <NAME>");
        }
    }
}
