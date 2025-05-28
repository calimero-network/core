use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use comfy_table::Table;
use eyre::bail;
use url::Url;

use crate::config::{Config, NodeConnection};

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

    /// Authentication key for the node
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
        let mut config = Config::load().await?;

        match self {
            NodeCommand::Add(cmd) => {
                let connection = match (cmd.path, cmd.url) {
                    (Some(path), None) => NodeConnection::Local { path },
                    (None, Some(url)) => NodeConnection::Remote {
                        url,
                        auth: cmd.auth,
                    },
                    _ => bail!("either `--path` or `--url` must be specified"),
                };
                let _ignored = config.nodes.insert(cmd.name, connection);
            }
            NodeCommand::Remove(cmd) => {
                let _ignored = config.nodes.remove(&cmd.name);
            }
            NodeCommand::List => {
                let mut table = Table::new();
                let _ignored = table.set_header(vec!["Name", "Type", "Location"]);

                for (name, conn) in &config.nodes {
                    match conn {
                        NodeConnection::Local { path, .. } => {
                            let _ignored = table.add_row(vec![name, "Local", path.as_str()]);
                        }
                        NodeConnection::Remote { url, .. } => {
                            let _ignored = table.add_row(vec![name, "Remote", url.as_str()]);
                        }
                    }
                }
                println!("{table}");
                return Ok(());
            }
        }

        config.save().await
    }
}
