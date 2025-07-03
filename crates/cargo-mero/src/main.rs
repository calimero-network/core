use clap::Parser;

mod abi;
mod build;
mod cli;
mod new;
mod utils;

async fn init() -> eyre::Result<()> {
    let command = cli::RootCommand::parse();

    command.run().await
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    init().await?;

    Ok(())
}
