use clap::{Parser, Subcommand};

use super::RootArgs;
use crate::cli::app::get::GetCommand;
use crate::cli::app::install::InstallCommand;
use crate::cli::app::list::ListCommand;
use crate::common::{ResponseBody, ToResponseBody};

pub(crate) mod get;
pub(crate) mod install;
pub(crate) mod list;

#[derive(Debug, Parser)]
pub struct AppCommand {
    #[command(subcommand)]
    pub subcommand: AppSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum AppSubCommands {
    Get(GetCommand),
    Install(InstallCommand),
    #[command(alias = "ls")]
    List(ListCommand),
}

impl AppCommand {
    pub async fn run(self, args: RootArgs) -> ResponseBody {
        match self.subcommand {
            AppSubCommands::Get(get) => get.run(&args).await.to_res_body(),
            AppSubCommands::Install(install) => install.run(&args).await.to_res_body(),
            AppSubCommands::List(list) => list.run(&args).await.to_res_body(),
        }
    }
}
