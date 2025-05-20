use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use comfy_table::Table;
use eyre::eyre;
use url::Url;

use crate::node_config::{NodeConfig, NodeConnection};

#[derive(Debug, Parser)]
pub struct AddNodeCommand {
    /// Name of the node
    pub name: String,

    /// Path to local node
    #[arg(long, conflicts_with = "url")]
    pub path: Option<Utf8PathBuf>,

    /// URL of remote node
    #[arg(long, conflicts_with = "path")]
    pub url: Option<Url>,

    /// Authentication key for the node (can also be set via MEROCTL_NODE_KEY env var)
    #[arg(long, env = "MEROCTL_NODE_KEY")]
    pub auth: Option<String>,
}

#[derive(Debug, Parser)]
pub struct RemoveNodeCommand {
    /// Name of the node to remove
    pub name: String,
}

#[derive(Debug, Subcommand)]
pub enum NodeCommand {
    /// Add or connect to a node
    #[command(alias = "connect")]
    Add(AddNodeCommand),

    /// Remove a node connection
    #[command(aliases = ["rm", "disconnect"])]
    Remove(RemoveNodeCommand),

    /// List all configured nodes
    #[command(alias = "ls")]
    List,
}

impl NodeCommand {
    pub async fn run(self) -> eyre::Result<()> {
        let mut config = NodeConfig::load()?;

        match self {
            NodeCommand::Add(cmd) => {
                let connection = match (cmd.path, cmd.url) {
                    (Some(path), None) => NodeConnection::Local { path },
                    (None, Some(url)) => NodeConnection::Remote {
                        url,
                        auth: cmd.auth,
                    },
                    _ => return Err(eyre!("Either --path or --url must be specified")),
                };
                drop(config.nodes.insert(cmd.name, connection));
            }
            NodeCommand::Remove(cmd) => {
                drop(config.nodes.remove(&cmd.name));
            }
            NodeCommand::List => {
                let mut table = Table::new();
                let _ = table.set_header(vec!["Name", "Type", "Location"]);

                for (name, conn) in &config.nodes {
                    match conn {
                        NodeConnection::Local { path, .. } => {
                            let _ = table.add_row(vec![name, "Local", path.as_str()]);
                        }
                        NodeConnection::Remote { url, .. } => {
                            let _ = table.add_row(vec![name, "Remote", url.as_str()]);
                        }
                    }
                }
                println!("{table}");
                return Ok(());
            }
        }

        config.save()?;
        Ok(())
    }
}
