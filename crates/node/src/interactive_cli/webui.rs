use calimero_network::config::DEFAULT_PORT;
use clap::Args;
use eyre::Result;
use libp2p::multiaddr::Protocol;
use primitives::common::multiaddr_to_url;
use webbrowser;

use crate::Node;

#[derive(Debug, Args)]
pub struct WebUICommand;

impl WebUICommand {
    pub fn run(&self, node: &Node) -> Result<()> {
        let addr = node
            .server_config
            .listen
            .get(0)
            .ok_or_else(|| eyre!("No listen address found"))?;
        let url: Url = multiaddr_to_url(addr, "/admin-dashboard").unwrap_or_else(|_| {
            Url::parse(&format!(
                "http://localhost:{}/admin-dashboard",
                DEFAULT_PORT
            ))
            .unwrap()
        });

        webbrowser::open(url.as_str()).map_err(|e| eyre!("Failed to open browser: {}", e))?;

        println!("Opened admin-dashboard at {}", url);

        Ok(())
    }
}
