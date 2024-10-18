#![allow(warnings)]

use crate::cli::RootCommand;
use clap::Parser;
use eyre::Result as EyreResult;

mod cli;

#[tokio::main]
async fn main() -> EyreResult<()> {
    let command = RootCommand::parse();
    command.run().await
}
