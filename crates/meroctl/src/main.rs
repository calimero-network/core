use std::process::ExitCode;

use clap::Parser;

use crate::cli::RootCommand;

mod cli;
mod common;
mod defaults;
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
