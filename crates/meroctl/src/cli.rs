use std::process::ExitCode;

use calimero_version::CalimeroVersion;
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Color, Table};
use const_format::concatcp;
use eyre::{bail, Report as EyreReport, Result};
use serde::{Serialize, Serializer};
use thiserror::Error as ThisError;
use url::Url;

use crate::client::Client;
use crate::common::{fetch_multiaddr, load_config, multiaddr_to_url};
use crate::config::Config;
use crate::connection::ConnectionInfo;
use crate::defaults;

use crate::output::{Format, Output, Report};
use calimero_client::ClientError;

mod app;
mod blob;
mod call;
mod context;
mod node;
mod peers;

use app::AppCommand;
use blob::BlobCommand;
use call::CallCommand;
use context::ContextCommand;
use node::NodeCommand;
use peers::PeersCommand;

use crate::auth::{authenticate_with_session_cache, check_authentication};

pub const EXAMPLES: &str = r"
  # List all applications
  $ meroctl --node node1 app ls

  # List all contexts
  $ meroctl --node node1 context ls

  # List all blobs
  $ meroctl --node node1 blob ls
";

#[derive(Debug, Parser)]
#[command(author, version = CalimeroVersion::current_str(), about, long_about = None)]
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
    Blob(BlobCommand),
    Context(ContextCommand),
    Call(CallCommand),
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

#[derive(Debug, Clone)]
pub struct Environment {
    pub output: Output,
    client: Option<Client>,
}

impl Environment {
    pub fn new(output: Output, connection: Option<ConnectionInfo>) -> Result<Self, CliError> {
        let client = if let Some(conn) = connection {
            Some(Client::new(conn)?)
        } else {
            None
        };

        Ok(Self { output, client })
    }

    pub fn client(&self) -> Result<&Client, CliError> {
        self.client
            .as_ref()
            .ok_or_else(|| CliError::Other(eyre::eyre!("Unable to create a connection.")))
    }
}

impl RootCommand {
    pub async fn run(self) -> Result<(), CliError> {
        let output = Output::new(self.args.output_format);

        // Some commands don't require a connection (like node commands)
        let needs_connection = match &self.action {
            SubCommands::Node(_) => false,
            _ => true,
        };

        let connection = if needs_connection {
            Some(self.prepare_connection(output).await?)
        } else {
            None
        };

        let mut environment = Environment::new(output, connection)?;

        let result = match self.action {
            SubCommands::App(application) => application.run(&mut environment).await,
            SubCommands::Blob(blob) => blob.run(&mut environment).await,
            SubCommands::Context(context) => context.run(&mut environment).await,
            SubCommands::Call(call) => call.run(&mut environment).await,
            SubCommands::Peers(peers) => peers.run(&mut environment).await,
            SubCommands::Node(node) => node.run(&environment).await,
        };

        if let Err(err) = result {
            let cli_err = match err.downcast::<ClientError>() {
                Ok(client_err) => CliError::ClientError(client_err),
                Err(err) => CliError::Other(err),
            };
            environment.output.write(&cli_err);
            return Err(cli_err);
        }

        Ok(())
    }

    // TODO: add custom error for handling authentication
    async fn prepare_connection(&self, output: Output) -> Result<ConnectionInfo> {
        if let Some(node) = &self.args.node {
            // Use specific node - first check if it's registered
            let config = Config::load().await?;

            if let Some(conn) = config.get_connection(node, output).await? {
                return Ok(conn);
            }

            // Check if it's a local node at <home>/<node>
            let config = load_config(&self.args.home, node).await?;
            let multiaddr = fetch_multiaddr(&config)?;
            let url = multiaddr_to_url(&multiaddr, "")?;

            // For unregistered local nodes, use authenticate_with_session_cache
            // This will handle authentication if needed, or bypass it for local nodes
            let connection = authenticate_with_session_cache(&url, &format!("local node {}", node), output).await?;
            Ok(connection)
        } else if let Some(api_url) = &self.args.api {
            // Use specific API URL - check session cache first, then authenticate if needed
            let connection =
                authenticate_with_session_cache(api_url, &api_url.to_string(), output).await?;
            Ok(connection)
        } else {
            // Try to use active node
            let config = Config::load().await?;

            if let Some(active_node_name) = &config.active_node {
                if let Some(conn) = config.get_connection(active_node_name, output).await? {
                    return Ok(conn);
                } else {
                    bail!(
                        "Active node '{}' not found. Please check your configuration.",
                        active_node_name
                    );
                }
            }

            // No active node set - fall back to default localhost server
            // For default localhost, use authenticate_with_session_cache
            let default_url = "http://127.0.0.1:2528".parse()?;
            let connection = authenticate_with_session_cache(&default_url, "default", output).await?;
            Ok(connection)
        }
    }
}

#[derive(Debug, Serialize, ThisError)]
pub enum CliError {
    #[error(transparent)]
    ClientError(#[from] ClientError),

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
            CliError::ClientError(_) => Self::from(101),
            CliError::Other(_) => Self::FAILURE,
        }
    }
}

impl Report for CliError {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("ERROR").fg(Color::Red)]);
        let _ = table.add_row(vec![match self {
            CliError::ClientError(e) => format!("Client Error: {}", e),
            CliError::Other(e) => format!("Error: {:?}", e),
        }]);
        println!("{table}");
    }
}

fn serialize_eyre_report<S>(report: &EyreReport, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.collect_seq(report.chain().map(|e| e.to_string()))
}
