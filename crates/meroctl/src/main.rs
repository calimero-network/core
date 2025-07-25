use std::process::ExitCode;

use clap::Parser;

pub mod cli;
mod common;
mod config;
mod connection;
mod defaults;
mod output;
mod version;

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(err) = color_eyre::install() {
        eprintln!("Failed to install color_eyre: {err}");
        return ExitCode::FAILURE;
    }

    version::check_for_update().await;

    let command = cli::RootCommand::parse();
    match command.run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => err.into(),
    }
}
