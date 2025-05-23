use std::env::var;

use clap::Parser;
use eyre::Result as EyreResult;
use tracing_subscriber::fmt::layer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{registry, EnvFilter};

mod cli;
mod defaults;
mod version;

use cli::RootCommand;
use version::check_for_update;

#[tokio::main]
async fn main() -> EyreResult<()> {
    setup()?;

    let command = RootCommand::parse();

    if rand::random::<u8>() % 10 == 0 {
        tokio::spawn(async move {
            if let Err(err) = check_for_update().await {
                eprintln!("Version check failed: {}", err);
            }
        });
    }

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
