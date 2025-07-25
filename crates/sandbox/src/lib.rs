use std::collections::HashMap;

pub mod config;
pub mod port_binding;
pub mod protocol;

use config::DevnetConfig;
use eyre::bail;
use port_binding::PortBinding;

use crate::protocol::ProtocolSandboxEnvironment;

#[derive(Debug)]
pub struct Devnet {
    config: DevnetConfig,
    pub nodes: HashMap<String, Node>,
    protocol_environments: HashMap<String, ProtocolSandboxEnvironment>,
}

#[derive(Debug)]
pub struct Node {
    pub name: String,
    pub swarm_addr: String,
    pub server_addr: String,
}

impl Devnet {
    pub fn new(config: DevnetConfig) -> eyre::Result<Self> {
        config.validate()?;

        Ok(Self {
            config,
            nodes: HashMap::new(),
            protocol_environments: HashMap::new(),
        })
    }

    pub async fn start(&mut self) -> eyre::Result<()> {
        self.init_protocol_environments().await?;
        self.start_nodes().await?;
        self.print_info();
        Ok(())
    }

    async fn init_protocol_environments(&mut self) -> eyre::Result<()> {
        for protocol in &self.config.protocols {
            if self.protocol_environments.contains_key(protocol) {
                continue;
            }

            let env = match protocol.as_str() {
                "near" => ProtocolSandboxEnvironment::Near(
                    protocol::near::NearSandboxEnvironment::init(
                        self.config.protocol_configs.near.clone(),
                    )
                    .await?,
                ),
                "icp" => {
                    ProtocolSandboxEnvironment::Icp(protocol::icp::IcpSandboxEnvironment::init(
                        self.config.protocol_configs.icp.clone(),
                    )?)
                }
                "stellar" => ProtocolSandboxEnvironment::Stellar(
                    protocol::stellar::StellarSandboxEnvironment::init(
                        self.config.protocol_configs.stellar.clone(),
                    )?,
                ),
                "ethereum" => ProtocolSandboxEnvironment::Ethereum(
                    protocol::ethereum::EthereumSandboxEnvironment::init(
                        self.config.protocol_configs.ethereum.clone(),
                    )?,
                ),
                _ => bail!("Unsupported protocol: {}", protocol),
            };

            self.protocol_environments.insert(protocol.clone(), env);
        }
        Ok(())
    }

    pub fn get_protocol_environment(
        &self,
        protocol: &str,
    ) -> eyre::Result<&ProtocolSandboxEnvironment> {
        self.protocol_environments
            .get(protocol)
            .ok_or_else(|| eyre::eyre!("Protocol {} not initialized", protocol))
    }

    async fn start_nodes(&mut self) -> eyre::Result<()> {
        let mut swarm_port = self.config.start_swarm_port;
        let mut server_port = self.config.start_server_port;

        for i in 0..self.config.node_count {
            let node_name = format!("node{}", i + 1);

            let swarm_binding =
                PortBinding::next_available(self.config.swarm_host.parse()?, &mut swarm_port)
                    .await?;
            let swarm_port_num = swarm_binding.port();

            let server_binding =
                PortBinding::next_available(self.config.server_host.parse()?, &mut server_port)
                    .await?;
            let server_port_num = server_binding.port();

            let node = Node {
                name: node_name.clone(),
                swarm_addr: format!("{}:{}", self.config.swarm_host, swarm_port_num),
                server_addr: format!("{}:{}", self.config.server_host, server_port_num),
            };

            self.nodes.insert(node_name, node);
        }

        Ok(())
    }

    fn print_info(&self) {
        println!("Devnet running with {} nodes:", self.nodes.len());

        for node in self.nodes.values() {
            println!("\nNode: {}", node.name);
            println!("  Swarm: {}", node.swarm_addr);
            println!("  RPC: {}", node.server_addr);
        }

        println!("\nProtocols enabled: {}", self.config.protocols.join(","));
    }
}
