use clap::Parser;
use color_eyre::eyre;
use tracing_subscriber::{prelude::*, EnvFilter};

mod cli;
mod config;
mod init;
mod network;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    setup()?;

    let command = cli::RootCommand::parse();

    match command.action {
        Some(cli::SubCommands::Init(init)) => init::run(command.args, init).await?,
        None => network::run(command.args).await?,
    }

    Ok(())
}

pub fn setup() -> eyre::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::builder().parse(format!(
            "chat_p0c=info,{}",
            std::env::var("RUST_LOG").unwrap_or_default()
        ))?)
        .with(tracing_subscriber::fmt::layer())
        .init();

    color_eyre::install()?;

    Ok(())
}
