use clap::Parser;

mod build;
mod cli;
mod new;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let command = cli::RootCommand::parse();
    command.run().await
}
