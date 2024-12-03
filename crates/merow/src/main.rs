#![allow(warnings)]
use clap::Parser;
use eyre::Result as EyreResult;
use merow::cli::RootCommand;

#[tokio::main]
async fn main() -> EyreResult<()> {
    let command = RootCommand::parse();
    command.run().await
}
