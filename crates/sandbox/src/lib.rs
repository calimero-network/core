use std::collections::HashMap;

use config::DevnetConfig;
use port_binding::PortBinding;

pub mod config;
pub mod protocol;

#[derive(Debug)]
pub struct Devnet {
    config: DevnetConfig,
    pub nodes: HashMap<String, Node>,
}

#[derive(Debug)]
pub struct Node {
    pub name: String,
    pub swarm_addr: String,
    pub server_addr: String,
}

impl Devnet {
    pub fn new(config: DevnetConfig) -> Self {
        Self {
            config,
            nodes: HashMap::new(),
        }
    }

    pub async fn start(&mut self) -> eyre::Result<()> {
        self.start_nodes().await?;
        self.print_info();
        Ok(())
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

pub mod port_binding {
    use std::net::{IpAddr, SocketAddr};

    use eyre::bail;
    use tokio::net::TcpListener;

    pub struct PortBinding {
        address: SocketAddr,
        listener: TcpListener,
    }

    impl PortBinding {
        pub async fn next_available(host: IpAddr, port: &mut u16) -> eyre::Result<PortBinding> {
            for _ in 0..100 {
                let address = (host, *port).into();

                let res = TcpListener::bind(address).await;

                *port += 1;

                if let Ok(listener) = res {
                    return Ok(PortBinding { address, listener });
                }
            }

            bail!(
                "unable to select a port in range {}..={}",
                *port - 100,
                *port - 1
            );
        }

        pub fn port(&self) -> u16 {
            self.address.port()
        }

        pub fn into_socket_addr(self) -> SocketAddr {
            drop(self.listener);
            self.address
        }
    }
}
