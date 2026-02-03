use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod get;
pub mod get_latest_version;
pub mod install;
pub mod list;
pub mod list_packages;
pub mod list_versions;
pub mod uninstall;
pub mod watch;

pub const EXAMPLES: &str = r"
  # List all applications
  $ meroctl --node node1 app ls

  # Get details of an application
  $ meroctl --node node1 app get <app_id>

  # Install an application with package/version
  $ meroctl --node node1 app install --package com.example.myapp --version 1.0.0 --path ./my-app.wasm

  # List all packages
  $ meroctl --node node1 app list-packages

  # List versions of a package
  $ meroctl --node node1 app list-versions com.example.myapp

  # Get latest version of a package
  $ meroctl --node node1 app get-latest-version com.example.myapp

  # Watch WASM file and update all contexts with the application
  $ meroctl --node node1 app watch --path ./my-app.wasm

  # Watch and update only contexts using a specific app (by app id)
  $ meroctl --node node1 app watch --path ./my-app.wasm --current-app-id <app_id>

  # Uninstall an application
  $ meroctl --node node1 app uninstall <app_id>
";

#[derive(Debug, Parser)]
#[command(about = "Command for managing applications")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct AppCommand {
    #[command(subcommand)]
    pub subcommand: AppSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum AppSubCommands {
    Get(get::GetCommand),
    Install(install::InstallCommand),
    #[command(alias = "ls")]
    List(list::ListCommand),
    Uninstall(uninstall::UninstallCommand),
    Watch(watch::WatchCommand),
    // Package management commands
    ListPackages(list_packages::ListPackagesCommand),
    ListVersions(list_versions::ListVersionsCommand),
    GetLatestVersion(get_latest_version::GetLatestVersionCommand),
}

impl AppCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            AppSubCommands::Get(get) => get.run(environment).await,
            AppSubCommands::Install(install) => install.run(environment).await,
            AppSubCommands::List(list) => list.run(environment).await,
            AppSubCommands::Uninstall(uninstall) => uninstall.run(environment).await,
            AppSubCommands::Watch(watch) => watch.run(environment).await,
            // Package management commands
            AppSubCommands::ListPackages(list_packages) => list_packages.run(environment).await,
            AppSubCommands::ListVersions(list_versions) => list_versions.run(environment).await,
            AppSubCommands::GetLatestVersion(get_latest_version) => {
                get_latest_version.run(environment).await
            }
        }
    }
}
