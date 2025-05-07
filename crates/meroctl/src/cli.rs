use std::process::ExitCode;

use bootstrap::BootstrapCommand;
use calimero_config::ConfigFile;
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Color, Table};
use const_format::concatcp;
use eyre::{eyre, Report as EyreReport};
use libp2p::Multiaddr;
use serde::{Serialize, Serializer};
use thiserror::Error as ThisError;
use url::Url;

use crate::common::{fetch_multiaddr, load_config};
use crate::defaults;
use crate::node_config::{NodeConfig, NodeConnection};
use crate::output::{Format, Output, Report};

mod app;
mod bootstrap;
mod call;
mod context;
mod peers;
mod proxy;

use app::AppCommand;
use call::CallCommand;
use context::ContextCommand;
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
    Connect(ConnectCommand),
}

#[derive(Debug, Parser)]
pub enum ConnectCommand {
    #[command(name = "local")]
    Local(LocalNodeCommand),

    #[command(name = "remote")]
    Remote(RemoteNodeCommand),
}

#[derive(Debug, Parser)]
pub struct LocalNodeCommand {
    pub alias: String,

    #[arg(long)]
    pub path: Utf8PathBuf,
}

#[derive(Debug, Parser)]
pub struct RemoteNodeCommand {
    pub alias: String,

    #[arg(long)]
    pub api: Url,
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[arg(long, value_name = "PATH", default_value_t = defaults::default_node_dir())]
    #[arg(env = "CALIMERO_HOME", hide_env_values = true)]
    pub home: Utf8PathBuf,

    /// Name of node
    #[arg(short, long, value_name = "NAME")]
    pub node_name: Option<String>,

    /// API endpoint URL
    #[arg(long, value_name = "URL", conflicts_with = "node_name")]
    pub api: Option<String>,

    /// Use a pre-configured node alias
    #[arg(long, value_name = "ALIAS", conflicts_with_all = &["node_name", "api"])]
    pub node: Option<String>,

    #[arg(long, value_name = "FORMAT", default_value_t, value_enum)]
    pub output_format: Format,
}

impl RootArgs {
    pub const fn new(
        home: Utf8PathBuf,
        node_name: Option<String>,
        api: Option<String>,
        node: Option<String>,
        output_format: Format,
    ) -> Self {
        Self {
            home,
            node_name,
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
        let connection = match (&self.args.node_name, &self.args.api, &self.args.node) {
            (Some(node_name), None, None) => {
                // Local node connection
                let config = load_config(&self.args.home, node_name)?;
                let multiaddr = fetch_multiaddr(&config).unwrap().clone();
                ConnectionInfo::Local { config, multiaddr }
            }
            (None, Some(api), None) => {
                // Direct API connection
                let api_url = api
                    .parse()
                    .map_err(|e| CliError::Other(eyre!("Invalid API URL: {}", e)))?;
                ConnectionInfo::Remote { api: api_url }
            }
            (None, None, Some(node_alias)) => {
                // Alias-based connection
                let node_config = NodeConfig::load().unwrap();
                match node_config.aliases.get(node_alias) {
                    Some(NodeConnection::Local { path }) => {
                        let config = load_config(path, node_alias)?;
                        let multiaddr = fetch_multiaddr(&config).unwrap().clone();
                        ConnectionInfo::Local { config, multiaddr }
                    }
                    Some(NodeConnection::Remote { api }) => {
                        ConnectionInfo::Remote { api: api.clone() }
                    }
                    None => return Err(CliError::Other(eyre!("Node alias not found"))),
                }
            }
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
            SubCommands::Connect(connect) => connect.run().await,
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

pub enum ConnectionInfo {
    Local {
        config: ConfigFile,
        multiaddr: Multiaddr,
    },
    Remote {
        api: Url,
    },
}

impl ConnectCommand {
    pub async fn run(self) -> eyre::Result<()> {
        let mut config =
            NodeConfig::load().map_err(|e| eyre!("Failed to load node config: {}", e))?;

        drop(match self {
            ConnectCommand::Local(LocalNodeCommand { alias, path }) => {
                config.aliases.insert(alias, NodeConnection::Local { path })
            }
            ConnectCommand::Remote(RemoteNodeCommand { alias, api }) => {
                config.aliases.insert(alias, NodeConnection::Remote { api })
            }
        });

        config
            .save()
            .map_err(|e| eyre!("Failed to save node config: {}", e))?;
        Ok(())
    }
}
