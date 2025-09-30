use clap::Parser;

mod build;
mod cli;
mod new;
mod version;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    version::check_for_update();
    let command = cli::RootCommand::parse();
    command.run().await
}
