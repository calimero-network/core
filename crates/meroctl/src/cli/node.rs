use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use comfy_table::Table;
use const_format::concatcp;
use eyre::{bail, Result};
use url::Url;

use crate::cli::check_authentication;
use crate::cli::storage::JwtToken;
use crate::common::{fetch_multiaddr, load_config, multiaddr_to_url};
use crate::config::{Config, NodeConnection};

#[derive(Debug, Parser)]
pub struct AddNodeCommand {
    /// Name of the node
    pub name: String,

    /// URL of remote node or path to local node directory
    pub location: String,

    /// Access token for authentication (skips automatic login)
    #[arg(long)]
    pub access_token: Option<String>,

    /// Refresh token for authentication (optional, used with access-token)
    #[arg(long)]
    pub refresh_token: Option<String>,
}

#[derive(Debug, Parser)]
pub struct RemoveNodeCommand {
    /// Name of the node to remove
    pub name: String,
}

#[derive(Debug, Parser)]
pub struct UseNodeCommand {
    /// Name of the node to set as active
    pub name: String,
}

pub const EXAMPLES: &str = r"
  # Add a local node
  $ meroctl node add node1 /path/to/home

  # Add another local node
  $ meroctl node add node2 ./my-node

  # Add a remote node (no authentication required)
  $ meroctl node add node3 http://public.node.com

  # Add a remote node that requires authentication (login will start automatically)
  $ meroctl node add node4 http://private.node.com

  # Add a remote node with explicit JWT tokens (skips automatic login)
  $ meroctl node add node5 http://private.node.com --access-token eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9... --refresh-token eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...

  # Add a remote node with just access token (no refresh capability)
  $ meroctl node add node6 http://private.node.com --access-token eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...

  # Set a node as active (default for commands)
  $ meroctl node use node1

  # Remove a node
  $ meroctl node remove node1
";

#[derive(Debug, Subcommand)]
#[command(about = "Command for managing nodes")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub enum NodeCommand {
    /// Add or connect to a node
    #[command(alias = "connect")]
    Add(AddNodeCommand),

    /// Remove a node connection
    #[command(aliases = ["rm", "disconnect"])]
    Remove(RemoveNodeCommand),

    /// Set a node as active (default for commands)
    Use(UseNodeCommand),

    /// List all configured nodes
    #[command(alias = "ls")]
    List,
}

impl NodeCommand {
    pub async fn run(self) -> Result<()> {
        let mut config = Config::load().await?;

        match self {
            NodeCommand::Add(cmd) => {
                let location_type = detect_location_type(&cmd.location)?;

                let connection = match location_type {
                    LocationType::Local(path) => {
                        let config = load_config(&path, &cmd.name).await?;
                        let multiaddr = fetch_multiaddr(&config)?;
                        let url = multiaddr_to_url(&multiaddr, "")?;

                        let jwt_tokens = determine_auth_tokens(
                            &cmd,
                            &url,
                            &format!("local node '{}'", cmd.name),
                        )
                        .await?;

                        NodeConnection::Local { path, jwt_tokens }
                    }
                    LocationType::Remote(url) => {
                        let jwt_tokens =
                            determine_auth_tokens(&cmd, &url, &format!("node '{}'", cmd.name))
                                .await?;

                        NodeConnection::Remote { url, jwt_tokens }
                    }
                };

                if config.nodes.contains_key(&cmd.name) {
                    bail!(
                        "node with name '{}' already exists, to update it, remove it first",
                        cmd.name
                    );
                }
                let _ignored = config.nodes.insert(cmd.name, connection);
            }
            NodeCommand::Remove(cmd) => {
                let _ignored = config.nodes.remove(&cmd.name);
                // If we're removing the active node, clear the active node
                if config.active_node.as_ref() == Some(&cmd.name) {
                    config.active_node = None;
                }
            }
            NodeCommand::Use(cmd) => {
                if !config.nodes.contains_key(&cmd.name) {
                    bail!(
                        "Node '{}' does not exist. Add it first with 'meroctl node add'",
                        cmd.name
                    );
                }
                config.active_node = Some(cmd.name.clone());
            }
            NodeCommand::List => {
                let mut table = Table::new();
                let _ignored = table.set_header(vec!["Name", "Type", "Location", "Active"]);

                for (name, conn) in &config.nodes {
                    let is_active = config.active_node.as_ref() == Some(name);
                    let active_marker = if is_active { "✓" } else { "" };

                    match conn {
                        NodeConnection::Local { path, .. } => {
                            let _ignored =
                                table.add_row(vec![name, "Local", path.as_str(), active_marker]);
                        }
                        NodeConnection::Remote { url, .. } => {
                            let _ignored =
                                table.add_row(vec![name, "Remote", url.as_str(), active_marker]);
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

#[derive(Debug, Clone)]
enum LocationType {
    Remote(Url),
    Local(Utf8PathBuf),
}

fn detect_location_type(location: &str) -> Result<LocationType> {
    // Try to parse as URL first
    if let Ok(url) = Url::parse(location) {
        // Check if it has a scheme that indicates it's a remote URL
        if url.scheme() == "http" || url.scheme() == "https" {
            return Ok(LocationType::Remote(url));
        }
    }

    // If not a valid remote URL, treat as local path
    let path = Utf8PathBuf::from(location);
    Ok(LocationType::Local(path))
}

async fn determine_auth_tokens(
    cmd: &AddNodeCommand,
    url: &Url,
    node_description: &str,
) -> Result<Option<JwtToken>> {
    // If access token is provided, use direct JWT tokens (skip automatic auth)
    if let Some(access_token) = &cmd.access_token {
        return Ok(Some(JwtToken {
            access_token: access_token.clone(),
            refresh_token: cmd.refresh_token.clone(),
        }));
    }

    // Otherwise, use automatic authentication
    check_authentication(url, node_description).await
}
