use std::process::ExitCode;

use clap::Parser;

use crate::cli::RootCommand;

mod cli;
mod common;
mod defaults;
pub mod node_config;
mod output;

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(err) = color_eyre::install() {
        eprintln!("Failed to install color_eyre: {err}");
        return ExitCode::FAILURE;
    }

    let command = RootCommand::parse();

    match command.run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => err.into(),
    }
}
