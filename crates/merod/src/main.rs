use std::env::var;

use calimero_utils_actix::init_global_runtime;
use clap::Parser;
use eyre::Result as EyreResult;
use tracing_subscriber::fmt::layer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{registry, EnvFilter};

mod cli;
mod defaults;
mod docker;
mod version;

use cli::RootCommand;

#[tokio::main]
async fn main() -> EyreResult<()> {
    setup()?;

    let command = RootCommand::parse();

    version::check_for_update();

    command.run().await
}

fn setup() -> EyreResult<()> {
    let directives = match var("RUST_LOG") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => "merod=info,calimero_=info".to_owned(),
    };

    registry()
        .with(EnvFilter::builder().parse(directives)?)
        .with(layer())
        .init();

    color_eyre::install()?;

    init_global_runtime()?;

    Ok(())
}
