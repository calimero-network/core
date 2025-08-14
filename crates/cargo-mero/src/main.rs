mod build;
mod cli;
mod new;

use clap::Parser;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let command = cli::RootCommand::parse();
    command.run().await
}
