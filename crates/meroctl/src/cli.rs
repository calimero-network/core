use std::process::ExitCode;

use calimero_version::CalimeroVersion;
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Color, Table};
use const_format::concatcp;
use eyre::{bail, OptionExt, Report as EyreReport};
use serde::{Serialize, Serializer};
use thiserror::Error as ThisError;
use url::Url;

use crate::common::{fetch_multiaddr, load_config, multiaddr_to_url};
use crate::config::Config;
use crate::connection::ConnectionInfo;
use crate::defaults;
use crate::output::{Format, Output, Report};

mod app;
mod auth;
mod call;
mod context;
mod node;
mod peers;
pub mod storage;

use app::AppCommand;
use auth::AuthCommand;
use call::CallCommand;
use context::ContextCommand;
use node::NodeCommand;
use peers::PeersCommand;
use storage::get_storage;

pub const EXAMPLES: &str = r"
  # List all applications
  $ meroctl --node node1 app ls

  # List all contexts
  $ meroctl --node node1 context ls
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
    Auth(AuthCommand),
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

#[derive(Debug)]
pub struct Environment {
    pub output: Output,
    connection: Option<ConnectionInfo>,
}

impl Environment {
    pub const fn new(output: Output, connection: Option<ConnectionInfo>) -> Self {
        Self { output, connection }
    }

    pub fn connection(&self) -> eyre::Result<&ConnectionInfo> {
        self.connection
            .as_ref()
            .ok_or_eyre("No node connection: either `--node` or `--api` must be set")
    }
}

impl RootCommand {
    pub async fn run(self) -> Result<(), CliError> {
        let output = Output::new(self.args.output_format);

        // Some commands don't require a connection (like auth commands)
        let needs_connection = match &self.action {
            SubCommands::Auth(_) => false,
            _ => true,
        };

        let connection = if needs_connection {
            match self.prepare_connection().await {
                Ok(conn) => conn,
                Err(err) => {
                    let err = CliError::Other(err);
                    output.write(&err);
                    return Err(err);
                }
            }
        } else {
            None
        };

        let environment = Environment::new(output, connection);

        let result = match self.action {
            SubCommands::App(application) => application.run(&environment).await,
            SubCommands::Auth(auth) => auth.run(&environment).await,
            SubCommands::Context(context) => context.run(&environment).await,
            SubCommands::Call(call) => call.run(&environment).await,
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

    async fn prepare_connection(&self) -> eyre::Result<Option<ConnectionInfo>> {
        match (&self.args.node, &self.args.api) {
            (Some(node), None) => {
                let config = Config::load().await?;

                if let Some(conn) = config.get_connection(node).await? {
                    return Ok(Some(conn));
                }

                let config = load_config(&self.args.home, node).await?;
                let multiaddr = fetch_multiaddr(&config)?;
                let url = multiaddr_to_url(&multiaddr, "")?;

                Ok(Some(ConnectionInfo::new(url, false)))
            }
            (None, Some(api_url)) => {
                let auth_required = check_auth_required(api_url).await?;
                if auth_required {
                    let storage = get_storage();

                    match storage.get_current_profile().await? {
                        Some((profile, profile_config)) => {
                            let profile_url_str =
                                profile_config.node_url.as_str().trim_end_matches('/');
                            let api_url_str = api_url.as_str().trim_end_matches('/');
                            if profile_url_str == api_url_str {
                                Ok(Some(ConnectionInfo::new(api_url.clone(), true)))
                            } else {
                                bail!("Current active profile '{}' is for {}, but you're trying to access {}.\nPlease login for this API: meroctl auth login --api {} --profile <profile_name>", 
                                      profile, profile_config.node_url, api_url, api_url);
                            }
                        }
                        None => {
                            bail!("Authentication required but no active profile found.\nPlease login first: meroctl auth login --api {} --profile <profile_name>", api_url);
                        }
                    }
                } else {
                    Ok(Some(ConnectionInfo::new(api_url.clone(), false)))
                }
            }
            // todo! if neither is selected, we should load the "default" config
            _ => Ok(None),
        }
    }
}

async fn check_auth_required(url: &Url) -> eyre::Result<bool> {
    let client = reqwest::Client::new();
    let health_url = url.join("/admin-api/health")?;

    match client.get(health_url).send().await {
        Ok(response) => match response.status().as_u16() {
            200..=299 => Ok(false),
            401 | 403 => Ok(true),
            _ => Ok(false),
        },
        Err(_) => Ok(false),
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
