use clap::Parser;

use crate::config;

#[derive(Debug, Parser)]
#[clap(author, about, version)]
pub struct RootCommand {
    #[clap(flatten)]
    pub args: RootArgs,
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[clap(long, value_name = "PATH", default_value_t = config::default_chat_dir())]
    #[clap(env = "CALIMERO_CHAT_HOME", hide_env_values = true)]
    pub home: camino::Utf8PathBuf,
}
