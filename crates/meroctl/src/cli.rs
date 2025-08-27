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

mod app;
pub mod auth;
mod blob;
mod call;
mod context;
mod node;
mod peers;
pub mod storage;

use app::AppCommand;
use auth::{authenticate_with_session_cache, check_authentication};
use blob::BlobCommand;
use call::CallCommand;
use context::ContextCommand;
use node::NodeCommand;
use peers::PeersCommand;

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
    connection: Option<ConnectionInfo>,
    mero_client: Option<Client>,
}

impl Environment {
    pub const fn new(output: Output, connection: Option<ConnectionInfo>) -> Self {
        Self {
            output,
            connection,
            mero_client: None,
        }
    }

    pub fn connection(&self) -> Result<&ConnectionInfo, CliError> {
        self.connection.as_ref().ok_or(CliError::Other(eyre::eyre!(
            "Unable to create a connection."
        )))
    }

    pub fn mero_client(&mut self) -> Result<&Client, CliError> {
        if self.mero_client.is_none() {
            let connection = self.connection()?;
            let mero_client = Client::new(connection.api_url.to_string())?;
            self.mero_client = Some(mero_client);
        }
        self.mero_client
            .as_ref()
            .ok_or_else(|| CliError::Other(eyre::eyre!("Failed to create mero client")))
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

        let mut environment = Environment::new(output, connection);

        let result = match self.action {
            SubCommands::App(application) => application.run(&mut environment).await,
            SubCommands::Blob(blob) => blob.run(&mut environment).await,
            SubCommands::Context(context) => context.run(&mut environment).await,
            SubCommands::Call(call) => call.run(&mut environment).await,
            SubCommands::Peers(peers) => peers.run(&mut environment).await,
            SubCommands::Node(node) => node.run(&environment).await,
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

            // Even local nodes might require authentication - use session cache for unregistered nodes
            let connection =
                authenticate_with_session_cache(&url, &format!("local node {}", node), output)
                    .await?;
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
            let default_url = "http://127.0.0.1:2528".parse()?;
            let connection =
                authenticate_with_session_cache(&default_url, "default localhost server", output)
                    .await?;
            Ok(connection)
        }
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
            CliError::Other(e) => format!("Error: {:?}", e),
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
    serializer.collect_seq(report.chain().map(|e| e.to_string()))
}
