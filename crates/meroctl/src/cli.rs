use std::process::ExitCode;

use bootstrap::BootstrapCommand;
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Color, Table};
use const_format::concatcp;
use eyre::{eyre, Report as EyreReport};
use libp2p::identity::Keypair;
use reqwest::Client;
use serde::{Serialize, Serializer};
use thiserror::Error as ThisError;
use tokio::time::Duration;
use url::Url;

use crate::common::{fetch_multiaddr, load_config, multiaddr_to_url};
use crate::config::Config;
use crate::defaults;
use crate::output::{ErrorLine, Format, Output, Report};

mod app;
mod bootstrap;
mod call;
mod context;
mod node;
mod peers;
mod proxy;

use app::AppCommand;
use call::CallCommand;
use context::ContextCommand;
use node::NodeCommand;
use peers::PeersCommand;
use proxy::ProxyCommand;

pub const EXAMPLES: &str = r"
  # List all applications
  $ meroctl --node-name node1 app ls
  # List all applications with custom destination config
  $ meroctl  --home data --node-name node1 app ls

  # List all contexts
  $ meroctl --node-name node1 context ls
  # List all contexts with custom destination config
  $ meroctl --home data --node-name node1 context ls
";

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
#[command(after_help = concatcp!(
    "Environment variables:\n",
    "  CALIMERO_HOME    Directory for config and data\n\n",
    "Examples:",
    EXAMPLES
))]
pub struct RootCommand {
    #[command(flatten)]
    pub args: RootArgs,

    #[command(subcommand)]
    pub action: SubCommands,
}

#[derive(Debug, Subcommand)]
pub enum SubCommands {
    App(AppCommand),
    Context(ContextCommand),
    Proxy(ProxyCommand),
    Call(CallCommand),
    Bootstrap(BootstrapCommand),
    Peers(PeersCommand),
    #[command(subcommand)]
    Node(NodeCommand),
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[arg(long, value_name = "PATH", default_value_t = defaults::default_node_dir())]
    #[arg(env = "CALIMERO_HOME", hide_env_values = true)]
    pub home: Utf8PathBuf,

    /// API endpoint URL
    #[arg(long, value_name = "URL")]
    pub api: Option<Url>,

    /// Use a pre-configured node alias
    #[arg(long, value_name = "ALIAS", conflicts_with = "api")]
    pub node: Option<String>,

    #[arg(long, value_name = "FORMAT", default_value_t, value_enum)]
    pub output_format: Format,
}

impl RootArgs {
    pub const fn new(
        home: Utf8PathBuf,
        api: Option<Url>,
        node: Option<String>,
        output_format: Format,
    ) -> Self {
        Self {
            home,
            api,
            node,
            output_format,
        }
    }
}

pub struct Environment {
    pub args: RootArgs,
    pub output: Output,
    pub connection: Option<ConnectionInfo>,
}

impl Environment {
    pub const fn new(args: RootArgs, output: Output, connection: Option<ConnectionInfo>) -> Self {
        Self {
            args,
            output,
            connection,
        }
    }
}

impl RootCommand {
    pub async fn run(self) -> Result<(), CliError> {
        let output = Output::new(self.args.output_format);

        // Determine connection info
        let connection = match (&self.args.node, &self.args.api) {
            (Some(node_name), None) => {
                // Check if node exists in config
                let node_config = Config::load()?;
                if let Some(conn) = node_config.nodes.get(node_name) {
                    match conn.get_connection_info(Some(node_name)).await {
                        Ok(info) => {
                            // Verify node is reachable
                            if let Err(e) = Self::check_node_ready(&info).await {
                                output.write(&ErrorLine(&format!("Node not ready: {}", e)));
                                return Err(e);
                            }
                            info
                        }
                        Err(e) => {
                            output.write(&ErrorLine(&format!("Failed to connect to node: {}", e)));
                            return Err(e);
                        }
                    }
                } else {
                    // Fall back to checking default home directory
                    let config = load_config(&defaults::default_node_dir(), node_name).await?;
                    let multiaddr = fetch_multiaddr(&config)?;
                    let url = multiaddr_to_url(&multiaddr, "")?;
                    ConnectionInfo {
                        api_url: url,
                        auth_key: Some(config.identity),
                    }
                }
            }
            (None, Some(api_url)) => ConnectionInfo {
                api_url: api_url.clone(),
                auth_key: std::env::var("MEROCTL_NODE_KEY")
                    .ok()
                    .and_then(|k| bs58::decode(k).into_vec().ok())
                    .and_then(|bytes| Keypair::from_protobuf_encoding(&bytes).ok()),
            },
            _ => return Err(CliError::Other(eyre!("Invalid connection parameters"))),
        };

        let environment = Environment::new(self.args, output, Some(connection));

        let result = match self.action {
            SubCommands::App(application) => application.run(&environment).await,
            SubCommands::Context(context) => context.run(&environment).await,
            SubCommands::Proxy(proxy) => proxy.run(&environment).await,
            SubCommands::Call(call) => call.run(&environment).await,
            SubCommands::Bootstrap(call) => call.run(&environment).await,
            SubCommands::Peers(peers) => peers.run(&environment).await,
            SubCommands::Node(node) => node.run().await,
        };

        if let Err(err) = result {
            let err = match err.downcast::<ApiError>() {
                Ok(err) => CliError::ApiError(err),
                Err(err) => CliError::Other(err),
            };
            environment.output.write(&err);
            return Err(err);
        }

        Ok(())
    }

    async fn check_node_ready(connection: &ConnectionInfo) -> Result<(), CliError> {
        let client = Client::new();
        let health_url = connection
            .api_url
            .join("health")
            .map_err(|e| CliError::Other(eyre!("Failed to construct health URL: {}", e)))?;

        let response = client
            .get(health_url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| CliError::Other(eyre!("Health check failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(CliError::Other(eyre!(
                "Node not healthy: HTTP {}",
                response.status()
            )));
        }

        Ok(())
    }
}

#[derive(Debug, Serialize, ThisError)]
pub enum CliError {
    #[error(transparent)]
    ApiError(#[from] ApiError),

    #[error(transparent)]
    Other(
        #[from]
        #[serde(serialize_with = "serialize_eyre_report")]
        EyreReport,
    ),
}

impl From<CliError> for ExitCode {
    fn from(error: CliError) -> Self {
        match error {
            CliError::ApiError(_) => Self::from(101),
            CliError::Other(_) => Self::FAILURE,
        }
    }
}

impl Report for CliError {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("ERROR").fg(Color::Red)]);
        let _ = table.add_row(vec![match self {
            CliError::ApiError(e) => format!("API Error ({}): {}", e.status_code, e.message),
            CliError::Other(e) => format!("Error: {}", e),
        }]);
        println!("{table}");
    }
}

#[derive(Debug, Serialize, ThisError)]
#[error("{status_code}: {message}")]
pub struct ApiError {
    pub status_code: u16,
    pub message: String,
}

fn serialize_eyre_report<S>(report: &EyreReport, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.collect_str(&report)
}

pub struct ConnectionInfo {
    pub api_url: Url,
    pub auth_key: Option<Keypair>,
}
