#![warn(unused_extern_crates)]
#![deny(unused_crate_dependencies)]

use clap::Parser;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

mod cli;
mod config;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    setup()?;

    let command = cli::RootCommand::parse();

    command.run().await
}

fn setup() -> eyre::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::builder().parse(format!(
            "info,{}",
            // "debug,libp2p_core=warn,libp2p_gossipsub=warn,{}",
            std::env::var("RUST_LOG").unwrap_or_default()
        ))?)
        .with(tracing_subscriber::fmt::layer())
        .init();

    color_eyre::install()
}
