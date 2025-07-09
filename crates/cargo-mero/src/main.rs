use clap::Parser;

mod build;
mod cli;
mod new;

async fn init() -> eyre::Result<()> {
    let command = cli::RootCommand::parse();

    command.run().await
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    init().await?;

    Ok(())
}
