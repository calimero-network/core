use std::process::ExitCode;

use clap::Parser;

mod cli;
mod common;
mod config;
mod defaults;
mod output;
mod version;

use cli::RootCommand;

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(err) = color_eyre::install() {
        eprintln!("Failed to install color_eyre: {err}");
        return ExitCode::FAILURE;
    }

    version::check_for_update().await;

    let command = RootCommand::parse();
    match command.run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => err.into(),
    }
}
