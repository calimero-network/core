use calimero_primitives::common::multiaddr_to_url;
use clap::Args;
use eyre::{eyre, Context, Result};
use webbrowser;

use crate::Node;

#[derive(Debug, Args)]
pub struct WebUICommand;

impl WebUICommand {
    pub fn run(&self, node: &Node) -> Result<()> {
        let addr = node
            .server_config
            .listen
            .first()
            .ok_or_else(|| eyre!("No listen address found"))?;
        let url = multiaddr_to_url(addr, "/admin-dashboard")
            .wrap_err("Failed to convert multiaddr to URL")?;

        webbrowser::open(url.as_str()).wrap_err("Failed to open browser")?;

        println!("Opened admin-dashboard at {}", url);
        Ok(())
    }
}
