use calimero_network::config::DEFAULT_PORT;
use clap::Args;
use eyre::Result;
use libp2p::multiaddr::Protocol;
use webbrowser;

use crate::Node;

#[derive(Debug, Args)]
pub struct WebUICommand;

impl WebUICommand {
    pub fn run(&self, node: &Node) -> Result<()> {
        let port = node
            .server_config
            .listen
            .iter()
            .find_map(|addr| {
                addr.iter().find_map(|proto| match proto {
                    Protocol::Tcp(port) => Some(port),
                    _ => None,
                })
            })
            .unwrap_or(DEFAULT_PORT);

        let url = format!("http://localhost:{}/admin-dashboard", port);
        webbrowser::open(&url).map_err(|e| eyre::eyre!("Failed to open browser: {}", e))?;
        println!("Opened admin-dashboard at {}", url);
        Ok(())
    }
}
