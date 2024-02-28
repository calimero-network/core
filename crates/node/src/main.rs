use clap::Parser;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

mod cli;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    setup()?;

    let command = cli::RootCommand::parse();

    command.run().await
}

pub fn setup() -> eyre::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::builder().parse(format!(
            "chat_p0c=info,{}",
            std::env::var("RUST_LOG").unwrap_or_default()
        ))?)
        .with(tracing_subscriber::fmt::layer())
        .init();

    color_eyre::install()
}
