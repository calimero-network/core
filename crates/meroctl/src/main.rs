use std::process::ExitCode;

use clap::Parser;
use rand::Rng;
use reqwest::Client;

use crate::cli::RootCommand;
use crate::version::check_for_update;

mod cli;
mod common;
mod defaults;
mod output;
mod version;

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(err) = color_eyre::install() {
        eprintln!("Failed to install color_eyre: {err}");
        return ExitCode::FAILURE;
    }

    if rand::random::<u8>() % 10 == 0 {
        if let Err(err) = check_for_update().await {
            eprintln!("Version check failed: {}", err);
        }
    }

    let command = RootCommand::parse();
    match command.run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => err.into(),
    }
}
