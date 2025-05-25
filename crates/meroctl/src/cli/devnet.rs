use std::sync::Arc;

use camino::Utf8PathBuf;
use clap::{arg, Parser, Subcommand};
use eyre::{Context, Result};
use mero_devnet::{Config, DevNetwork};
use tokio::sync::Mutex;

use crate::cli::Environment;

// Global state for the running network
static RUNNING_NETWORK: Mutex<Option<Arc<Mutex<DevNetwork>>>> = Mutex::const_new(None);

#[derive(Debug, Parser)]
pub struct DevnetCommand {
    #[command(subcommand)]
    pub action: DevnetSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum DevnetSubCommands {
    /// Start a local development network
    Start {
        /// Path to config file
        #[arg(short, long)]
        config: Option<Utf8PathBuf>,

        /// Path to merod binary
        #[arg(short, long)]
        binary: Utf8PathBuf,

        /// Directory for logs
        #[arg(short, long)]
        logs_dir: Utf8PathBuf,
    },
    /// Stop the running development network
    Stop,
    /// Show status of the development network
    Status,
}

impl DevnetCommand {
    pub async fn run(self, _env: &Environment) -> Result<()> {
        match self.action {
            DevnetSubCommands::Start {
                config,
                binary,
                logs_dir,
            } => Self::start(config, binary, logs_dir).await,
            DevnetSubCommands::Stop => Self::stop().await,
            DevnetSubCommands::Status => Self::status().await,
        }
    }

    async fn start(
        config_path: Option<Utf8PathBuf>,
        binary: Utf8PathBuf,
        logs_dir: Utf8PathBuf,
    ) -> Result<()> {
        {
            let running_network = RUNNING_NETWORK.lock().await;
            if running_network.is_some() {
                eyre::bail!("A devnet is already running. Stop it first or check status.");
            }
        }

        let config_path = config_path.unwrap_or_else(|| Utf8PathBuf::from("devnet-config.json"));

        let config_content = tokio::fs::read_to_string(&config_path)
            .await
            .context("Failed to read config file")?;
        let config: Config =
            serde_json::from_str(&config_content).context("Failed to parse config file")?;

        let network = DevNetwork::new(config, binary, logs_dir)
            .await
            .context("Failed to initialize devnet")?;

        let network = Arc::new(Mutex::new(network));

        {
            let mut running_network = RUNNING_NETWORK.lock().await;
            *running_network = Some(Arc::clone(&network));
        }

        network
            .lock()
            .await
            .start()
            .await
            .context("Failed to start devnet")?;

        println!("Devnet started successfully. Press Ctrl+C to stop.");

        tokio::signal::ctrl_c().await?;

        Self::stop().await?;
        Ok(())
    }

    async fn stop() -> Result<()> {
        let network = {
            let mut running_network = RUNNING_NETWORK.lock().await;
            running_network.take()
        };

        if let Some(network) = network {
            network
                .lock()
                .await
                .stop()
                .await
                .context("Failed to stop devnet")?;
            println!("Devnet stopped successfully");
        } else {
            println!("No devnet is currently running");
        }

        Ok(())
    }

    async fn status() -> Result<()> {
        let running_network = RUNNING_NETWORK.lock().await;

        if let Some(network) = &*running_network {
            let network = network.lock().await;
            println!("Devnet is running");
            println!("Nodes:");
            for node in network.nodes() {
                let node = node.lock().await;
                println!("- {} (home: {})", node.name, node.home_dir);
            }
            println!("Protocols:");
            for protocol in network.protocol_envs() {
                println!("- {}", protocol.name());
            }
        } else {
            println!("No devnet is currently running");
        }

        Ok(())
    }
}
