use std::process::ExitCode;

use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Report as EyreReport;
use serde::{Serialize, Serializer};
use thiserror::Error as ThisError;

use crate::defaults;
use crate::output::{Format, Output, Report};

mod app;
mod context;
mod jsonrpc;

use app::AppCommand;
use context::ContextCommand;
use jsonrpc::CallCommand;

pub const EXAMPLES: &str = r"
  # List all applications
  $ meroctl -- --node-name node1 app ls

  # List all contexts
  $ meroctl -- --home data --node-name node1 context ls
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
    Context(ContextCommand),
    App(AppCommand),
    JsonRpc(CallCommand),
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[arg(long, value_name = "PATH", default_value_t = defaults::default_node_dir())]
    #[arg(env = "CALIMERO_HOME", hide_env_values = true)]
    pub home: Utf8PathBuf,

    /// Name of node
    #[arg(short, long, value_name = "NAME")]
    pub node_name: String,

    #[arg(long, value_name = "FORMAT")]
    pub output_format: Format,
}

pub struct Environment {
    pub args: RootArgs,
    pub output: Output,
}

impl Environment {
    pub const fn new(args: RootArgs, output: Output) -> Self {
        Self { args, output }
    }
}

impl RootCommand {
    pub async fn run(self) -> Result<(), CliError> {
        let output = Output::new(self.args.output_format);
        let environment = Environment::new(self.args, output);

        let result = match self.action {
            SubCommands::Context(context) => context.run(&environment).await,
            SubCommands::App(application) => application.run(&environment).await,
            SubCommands::JsonRpc(jsonrpc) => jsonrpc.run(&environment).await,
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
        println!("{self}");
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
