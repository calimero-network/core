use std::env::var;

use clap::Parser;
use eyre::Result as EyreResult;
use tracing_subscriber::fmt::layer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{registry, EnvFilter};

use crate::cli::RootCommand;

mod cli;
mod defaults;
mod config; // Make sure config.rs is included

#[tokio::main]
async fn main() -> EyreResult<()> {
    setup()?;

    // Parse the root command
    let command = RootCommand::parse();

    // Execute the parsed command (which includes `ConfigCommand`)
    command.run().await
}

fn setup() -> EyreResult<()> {
    registry()
        .with(EnvFilter::builder().parse(format!(
            "merod=info,calimero_=info,{}",
            var("RUST_LOG").unwrap_or_default()
        ))?)
        .with(layer())
        .init();

    color_eyre::install()
}
