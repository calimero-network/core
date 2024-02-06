use std::net::IpAddr;

use clap::{Parser, Subcommand, ValueEnum};
use libp2p::Multiaddr;

use crate::config;

#[derive(Debug, Parser)]
#[clap(author, about, version)]
pub struct RootCommand {
    #[clap(flatten)]
    pub args: RootArgs,

    #[clap(subcommand)]
    pub action: Option<SubCommands>,
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[clap(long, value_name = "PATH", default_value_t = config::default_chat_dir())]
    #[clap(env = "CALIMERO_CHAT_HOME", hide_env_values = true)]
    pub home: camino::Utf8PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum SubCommands {
    Init(InitCommand),
}

#[derive(Debug, Parser)]
/// Initialize node configuration
pub struct InitCommand {
    /// List of bootstrap nodes
    #[clap(long, value_name = "ADDR")]
    pub boot_nodes: Vec<Multiaddr>,

    /// Use nodes from a known network
    #[clap(long, value_name = "NETWORK")]
    pub boot_network: Option<BootstrapNodes>,

    /// Host to listen on
    #[clap(long, value_name = "IP")]
    #[clap(default_value = "0.0.0.0,::")]
    #[clap(use_value_delimiter = true)]
    pub host: Vec<IpAddr>,

    /// Port to listen on
    #[clap(long, value_name = "PORT")]
    #[clap(default_value_t = config::DEFAULT_PORT)]
    pub port: u16,

    /// Enable mDNS discovery
    #[clap(long, default_value_t = true)]
    #[clap(overrides_with("no_mdns"))]
    pub mdns: bool,

    #[clap(long, hide = true)]
    #[clap(overrides_with("mdns"))]
    pub no_mdns: bool,

    /// Force initialization even if the directory already exists
    #[clap(long)]
    pub force: bool,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum BootstrapNodes {
    Ipfs,
}
