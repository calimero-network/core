use std::process::ExitCode;

use calimero_version::CalimeroVersion;
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Color, Table};
use const_format::concatcp;
use eyre::{bail, Report as EyreReport};
use libp2p::identity::Keypair;
use serde::{Serialize, Serializer};
use thiserror::Error as ThisError;
use url::Url;

use crate::common::{fetch_multiaddr, load_config, multiaddr_to_url};
use crate::config::Config;
use crate::connection::ConnectionInfo;
use crate::defaults;
use crate::output::{Format, Output, Report};

mod app;
mod bootstrap;
mod call;
mod context;
mod node;
mod peers;

use app::AppCommand;
use bootstrap::BootstrapCommand;
use call::CallCommand;
use context::ContextCommand;
use node::NodeCommand;
use peers::PeersCommand;

pub const EXAMPLES: &str = r"
  # Authentication examples
  $ meroctl auth login                    # Login with default profile
  $ meroctl auth login --profile prod     # Login with specific profile  
  $ meroctl auth status                   # Show auth status
  $ meroctl auth logout                   # Logout from default profile

  # Using with authentication
  $ meroctl --api https://node.calimero.network context ls    # Auto-auth
  $ meroctl --profile prod --api https://node.example.com app ls

  # Using environment variables  
  $ MEROCTL_TOKEN=xyz meroctl --api https://node.example.com context ls
  $ MEROCTL_PROFILE=prod meroctl --api https://node.example.com app ls

  # Development mode (no auth)
  $ meroctl --no-auth --api http://localhost:2428 context ls
";

#[derive(Debug, Parser)]
#[command(author, version = CalimeroVersion::current_str(), about, long_about = None)]
#[command(after_help = concatcp!(
    "Environment variables:\n",
    "  CALIMERO_HOME     Directory for config and data\n",
    "  MEROCTL_TOKEN     JWT token for authentication\n",
    "  MEROCTL_PROFILE   Authentication profile to use\n\n",
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
    Call(CallCommand),
    Bootstrap(BootstrapCommand),
    Peers(PeersCommand),
    #[command(subcommand)]
    Node(NodeCommand),
    /// Manage authentication
    Auth(crate::auth::AuthCommand),
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

    /// Authentication profile to use
    #[arg(long, value_name = "PROFILE", env = "MEROCTL_PROFILE")]
    pub profile: Option<String>,

    /// JWT token for authentication (overrides stored tokens)
    #[arg(long, value_name = "TOKEN", env = "MEROCTL_TOKEN")]
    pub token: Option<String>,

    /// Skip authentication (for development mode)
    #[arg(long)]
    pub no_auth: bool,
}

impl RootArgs {
    pub const fn new(
        home: Utf8PathBuf,
        api: Option<Url>,
        node: Option<String>,
        output_format: Format,
        profile: Option<String>,
        token: Option<String>,
        no_auth: bool,
    ) -> Self {
        Self {
            home,
            api,
            node,
            output_format,
            profile,
            token,
            no_auth,
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

        let connection = match self.prepare_connection().await {
            Ok(conn) => conn,
            Err(err) => {
                let err = CliError::Other(err);
                output.write(&err);
                return Err(err);
            }
        };

        let environment = Environment::new(self.args, output, Some(connection));

        let result = match self.action {
            SubCommands::App(application) => application.run(&environment).await,
            SubCommands::Context(context) => context.run(&environment).await,
            SubCommands::Call(call) => call.run(&environment).await,
            SubCommands::Bootstrap(call) => call.run(&environment).await,
            SubCommands::Peers(peers) => peers.run(&environment).await,
            SubCommands::Node(node) => node.run().await,
            SubCommands::Auth(auth) => auth.run(&environment).await,
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

    async fn prepare_connection(&self) -> eyre::Result<ConnectionInfo> {
        // Get the API URL
        let api_url = match (&self.args.node, &self.args.api) {
            (Some(node), None) => {
                // Try to get from stored config first
                let config = Config::load().await?;
                if let Some(conn) = config.get_connection(node).await? {
                    return Ok(conn);
                }

                // Fallback to local node config
                let config = load_config(&self.args.home, node).await?;
                let multiaddr = fetch_multiaddr(&config)?;
                multiaddr_to_url(&multiaddr, "")?
            }
            (None, Some(api_url)) => api_url.clone(),
            _ => bail!("expected one of `--node` or `--api` to be set"),
        };

        // Handle authentication
        if self.args.no_auth {
            // Development mode - no authentication
            return Ok(ConnectionInfo::new(api_url, None).await);
        }

        // Check for direct token first
        if let Some(token) = &self.args.token {
            return Ok(ConnectionInfo::new_with_jwt(api_url, token.clone()).await);
        }

        // Set up auth manager for JWT authentication
        let profile = self.args.profile.clone().unwrap_or_else(|| "default".to_string());
        let auth_manager = crate::auth::AuthManager::new(profile, api_url.clone()).await?;
        
        // Check if we have valid tokens already
        if let Ok(Some(token)) = auth_manager.get_valid_token().await {
            return Ok(ConnectionInfo::new_with_jwt(api_url, token).await);
        }

        // Return connection with auth manager (will handle auth on-demand)
        Ok(ConnectionInfo::new_with_auth_manager(api_url, auth_manager).await)
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
