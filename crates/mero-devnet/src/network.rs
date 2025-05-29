use std::sync::Arc;

use camino::Utf8PathBuf;
use eyre::{Context, Result};
use tokio::fs::create_dir_all;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

use crate::merod::Merod;
// use crate::output::OutputWriter;
use crate::protocol::ProtocolSandboxEnvironment;
use crate::{Config, ProtocolSandboxConfig};

pub struct DevNetwork {
    nodes: Vec<Arc<Mutex<Merod>>>,
    protocol_envs: Vec<ProtocolSandboxEnvironment>,
    start_time: Instant,
}

impl DevNetwork {
    pub async fn new(
        config: Config,
        binary_path: Utf8PathBuf,
        logs_dir: Utf8PathBuf,
        // requested_protocols: Option<&[String]>,
        test_id: u32,
        // output_writer: OutputWriter,
    ) -> Result<Self> {
        let mut nodes = Vec::with_capacity(config.network.node_count as usize);
        let mut protocol_envs = Vec::with_capacity(config.protocol_sandboxes.len());

        // Initialize protocol environments
        for protocol in config.protocol_sandboxes.iter() {
            let env = match protocol {
                ProtocolSandboxConfig::Near(cfg) => ProtocolSandboxEnvironment::Near(
                    crate::protocol::near::NearSandboxEnvironment::init(cfg.clone()).await?,
                ),
                ProtocolSandboxConfig::Icp(cfg) => ProtocolSandboxEnvironment::Icp(
                    crate::protocol::icp::IcpSandboxEnvironment::init(cfg.clone())?,
                ),
                ProtocolSandboxConfig::Stellar(cfg) => ProtocolSandboxEnvironment::Stellar(
                    crate::protocol::stellar::StellarSandboxEnvironment::init(cfg.clone())?,
                ),
                ProtocolSandboxConfig::Ethereum(cfg) => ProtocolSandboxEnvironment::Ethereum(
                    crate::protocol::ethereum::EthereumSandboxEnvironment::init(cfg.clone())?,
                ),
            };
            protocol_envs.push(env);
        }

        // Initialize nodes
        for i in 0..config.network.node_count {
            let node_name = format!("node{}", i + 1);
            let home_dir = logs_dir.join(&node_name);

            // Ensure home directory exists
            create_dir_all(&home_dir).await?;

            // Create complete config file if it doesn't exist
            let config_file = home_dir.join("config.json");
            if !config_file.exists() {
                let default_config = serde_json::json!({
                    "network": {
                        "node_name": &node_name,
                        "swarm_host": config.network.swarm_host.to_string(),
                        "server_host": config.network.server_host.to_string(),
                        "swarm_port": config.network.start_swarm_port + i as u16,
                        "server_port": config.network.start_server_port + i as u16,
                        "rendezvous_namespace": format!("calimero/e2e-tests/{}", test_id),
                        "discovery": {
                            "rendezvous": {
                                "namespace": format!("calimero/e2e-tests/{}", test_id)
                            }
                        }
                    },
                    "storage": {
                        "path": home_dir.join("data").to_string()
                    }
                });
                tokio::fs::write(&config_file, serde_json::to_string_pretty(&default_config)?)
                    .await?;
            }

            let merod = Merod::new(
                node_name.clone(),
                home_dir,
                &logs_dir,
                binary_path.clone(),
                Default::default(),
            );

            let swarm_port = config.network.start_swarm_port + i as u16;
            let server_port = config.network.start_server_port + i as u16;

            merod
                .init(
                    &config.network.swarm_host.to_string(),
                    &config.network.server_host.to_string(),
                    swarm_port,
                    server_port,
                    config.merod.args.iter().map(|s| s.as_str()),
                )
                .await
                .context(format!("Failed to initialize node {}", node_name))?;

            nodes.push(Arc::new(Mutex::new(merod)));
        }

        Ok(Self {
            nodes,
            protocol_envs,
            start_time: Instant::now(),
        })
    }

    pub async fn start(&self) -> Result<()> {
        for node in &self.nodes {
            node.lock().await.run().await?;
        }
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        for node in &self.nodes {
            node.lock().await.stop().await?;
        }
        Ok(())
    }

    pub fn nodes(&self) -> &[Arc<Mutex<Merod>>] {
        &self.nodes
    }

    pub fn protocol_envs(&self) -> &[ProtocolSandboxEnvironment] {
        &self.protocol_envs
    }

    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }

    pub fn is_running(&self) -> bool {
        // Could add more sophisticated checks here
        true
    }
}
