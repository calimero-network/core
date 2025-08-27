use std::process::ExitCode;

use clap::Parser;

mod auth;
pub mod cli;
mod client;
mod common;
mod config;
mod connection;
mod defaults;
mod errors;
mod output;
mod storage;
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
