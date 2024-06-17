use clap::{Parser, Subcommand};

use crate::config;

mod init;
mod link;
mod run;
mod setup;
#[derive(Debug, Parser)]
#[command(author, about, version)]
pub struct RootCommand {
    #[command(flatten)]
    pub args: RootArgs,

    #[command(subcommand)]
    pub action: SubCommands,
}

#[derive(Debug, Subcommand)]
pub enum SubCommands {
    Init(init::InitCommand),
    Setup(setup::SetupCommand),
    Run(run::RunCommand),
    Link(link::LinkCommand),
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[arg(long, value_name = "PATH", default_value_t = config::default_chat_dir())]
    #[arg(env = "CALIMERO_HOME", hide_env_values = true)]
    pub home: camino::Utf8PathBuf,
}

impl RootCommand {
    pub async fn run(self) -> eyre::Result<()> {
        let _c = RootCommand::parse();
        match self.action {
            SubCommands::Init(init) => return init.run(self.args),
            SubCommands::Setup(setup) => return setup.run(self.args),
            SubCommands::Run(run) => return run.run(self.args).await,
            SubCommands::Link(link) => link.run(self.args),
        }
    }
}
