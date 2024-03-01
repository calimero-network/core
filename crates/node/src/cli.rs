use calimero_node::config;
use clap::{Parser, Subcommand};

mod init;
mod run;

#[derive(Debug, Parser)]
#[clap(author, about, version)]
pub struct RootCommand {
    #[clap(flatten)]
    pub args: RootArgs,

    #[clap(subcommand)]
    pub action: SubCommands,
}

#[derive(Debug, Subcommand)]
pub enum SubCommands {
    Init(init::InitCommand),
    Run(run::RunCommand),
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[clap(long, value_name = "PATH", default_value_t = config::default_chat_dir())]
    #[clap(env = "CALIMERO_CHAT_HOME", hide_env_values = true)]
    pub home: camino::Utf8PathBuf,
}

impl RootCommand {
    pub async fn run(self) -> eyre::Result<()> {
        match self.action {
            SubCommands::Init(init) => return init.run(self.args),
            SubCommands::Run(run) => return run.run(self.args).await,
        }
    }
}
