use calimero_node::config::{self, ConfigFile};
use clap::{Parser, Subcommand, ValueEnum};
use color_eyre::eyre;

mod init;

#[derive(Debug, Parser)]
#[clap(author, about, version)]
pub struct RootCommand {
    #[clap(flatten)]
    pub args: RootArgs,

    #[clap(subcommand)]
    pub action: Option<SubCommands>,
}

#[derive(Debug, Subcommand)]
pub enum SubCommands {
    Init(init::InitCommand),
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[clap(long, value_name = "PATH", default_value_t = config::default_chat_dir())]
    #[clap(env = "CALIMERO_CHAT_HOME", hide_env_values = true)]
    pub home: camino::Utf8PathBuf,

    #[clap(long, value_name = "TYPE")]
    #[clap(value_enum, default_value_t)]
    pub node_type: NodeType,
}

#[derive(Clone, Debug, Default, ValueEnum)]
pub enum NodeType {
    #[default]
    Peer,
    Coordinator,
}

impl From<NodeType> for calimero_primitives::types::NodeType {
    fn from(value: NodeType) -> Self {
        match value {
            NodeType::Peer => calimero_primitives::types::NodeType::Peer,
            NodeType::Coordinator => calimero_primitives::types::NodeType::Coordinator,
        }
    }
}

impl RootCommand {
    pub async fn run(self) -> eyre::Result<()> {
        match self.action {
            Some(sub_command) => {
                match sub_command {
                    SubCommands::Init(init) => init.run(self.args)?,
                };
                return Ok(());
            }
            None => {}
        }

        if !ConfigFile::exists(&self.args.home) {
            eyre::bail!("chat node is not initialized in {:?}", self.args.home);
        }

        let config = ConfigFile::load(&self.args.home)?;

        calimero_node::start(calimero_node::NodeConfig {
            home: self.args.home,
            node_type: self.args.node_type.into(),
            network: calimero_network::config::NetworkConfig {
                identity: config.identity,
                swarm: config.network.swarm,
                bootstrap: config.network.bootstrap,
                discovery: config.network.discovery,
                endpoint: config.network.endpoint,
            },
        })
        .await
    }
}
