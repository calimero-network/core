//! Standalone Calimero relayer binary
//!
//! This binary provides a standalone relayer service that can be run
//! independently of the main merod node.

use std::env::var;

use calimero_relayer::{addr_from_str, RelayerConfig, RelayerService, DEFAULT_ADDR};
use camino::Utf8PathBuf;
use clap::Parser;
use color_eyre::install;
use dirs::home_dir;
use eyre::Result as EyreResult;
use tracing_subscriber::fmt::layer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{registry, EnvFilter};

const DEFAULT_CALIMERO_HOME: &str = ".calimero";

/// Standalone Calimero relayer
#[derive(Debug, Parser)]
#[command(
    name = "calimero-relayer",
    about = "Standalone Calimero relayer for external client interactions",
    version = env!("CARGO_PKG_VERSION")
)]
struct Cli {
    /// Sets the address to listen on [default: 0.0.0.0:63529]
    /// Valid: `63529`, `127.0.0.1`, `127.0.0.1:63529` [env: PORT]
    #[clap(short, long, value_name = "URI")]
    #[clap(verbatim_doc_comment, value_parser = addr_from_str)]
    #[clap(default_value_t = DEFAULT_ADDR)]
    pub listen: std::net::SocketAddr,

    /// Directory for config and data
    #[arg(long, value_name = "PATH", default_value_t = default_node_dir())]
    #[arg(env = "CALIMERO_HOME", hide_env_values = true)]
    pub home: Utf8PathBuf,

    /// Name of node
    #[arg(short, long, value_name = "NAME")]
    pub node_name: Utf8PathBuf,
}

fn default_node_dir() -> Utf8PathBuf {
    if let Some(home) = home_dir() {
        let home = camino::Utf8Path::from_path(&home).expect("invalid home directory");
        return home.join(DEFAULT_CALIMERO_HOME);
    }

    Utf8PathBuf::default()
}

#[tokio::main]
async fn main() -> EyreResult<()> {
    setup()?;

    let cli = Cli::parse();

    let node_path = cli.home.join(cli.node_name);
    let config = RelayerConfig::new(cli.listen, node_path);
    let service = RelayerService::new(config);

    service.start().await
}

fn setup() -> EyreResult<()> {
    registry()
        .with(EnvFilter::builder().parse(format!(
            "calimero_relayer=info,calimero_=info,{}",
            var("RUST_LOG").unwrap_or_default()
        ))?)
        .with(layer())
        .init();

    install()?;

    Ok(())
}
