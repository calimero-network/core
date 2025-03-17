use std::env::var;

use calimero_utils_actix::init_global_runtime;
use clap::Parser;
use eyre::Result as EyreResult;
use tracing_subscriber::fmt::layer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{registry, EnvFilter};

use crate::cli::RootCommand;

mod cli;
mod defaults;

#[tokio::main]
async fn main() -> EyreResult<()> {
    setup()?;

    let command = RootCommand::parse();

    command.run().await
}

fn setup() -> EyreResult<()> {
    registry()
        .with(EnvFilter::builder().parse(format!("info,{}", var("RUST_LOG").unwrap_or_default()))?)
        .with(layer())
        .init();

    color_eyre::install()?;

    init_global_runtime()?;

    Ok(())
}
