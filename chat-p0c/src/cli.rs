use clap::{Parser, Subcommand};
use libp2p::Multiaddr;

// cp0c init
// cp0c --listen 2428 --bootstrap /ip4/127.0.0.1/tcp/2428 --bootstrap /ip4/127.0.0.1/tcp/2429,/ip4/127.0.0.1/tcp/2430 --identity /path/to/identity

pub const DEFAULT_CALIMERO_CHAT_HOME: &str = ".calimero/experiments/chat-p0c";

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
    #[clap(long, value_name = "PATH", default_value_t = default_chat_dir())]
    #[clap(env = "CALIMERO_CHAT_HOME", hide_env_values = true)]
    pub home: camino::Utf8PathBuf,
}

pub fn default_chat_dir() -> camino::Utf8PathBuf {
    if let Some(home) = dirs::home_dir() {
        let home = camino::Utf8Path::from_path(&home).expect("invalid home directory");
        return home.join(DEFAULT_CALIMERO_CHAT_HOME);
    }

    Default::default()
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
