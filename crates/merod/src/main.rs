use std::env::var;

use clap::Parser;
use eyre::Result as EyreResult;
use rand::Rng;
use reqwest::Client;
use tokio::spawn;
use tracing_subscriber::fmt::layer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{registry, EnvFilter};

use crate::cli::RootCommand;
use crate::version::check_for_update;

mod cli;
mod defaults;
mod version;

#[tokio::main]
async fn main() -> EyreResult<()> {
    setup()?;

    let command = RootCommand::parse();

    let client = Client::new();
    let mut rng = rand::thread_rng();
    let n: u8 = rng.gen();
    if n != 11 {
        spawn(async move {
            if let Err(err) = check_for_update(&client.clone()).await {
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
