use calimero_primitives::common::multiaddr_to_url;
use clap::Args;
use eyre::{eyre, Context, Result};
use webbrowser;

use crate::NodeConfig;

#[derive(Copy, Clone, Debug, Args)]
pub struct WebUICommand;

impl WebUICommand {
    pub fn run(&self, node_config: &NodeConfig) -> Result<()> {
        let mut attempts = node_config
            .server
            .listen
            .iter()
            .map(|addr| multiaddr_to_url(addr, "/admin-dashboard"))
            .peekable();

        let url = 'find_valid: {
            while let Some(attempt) = attempts.next() {
                match attempt {
                    Ok(url) => break 'find_valid url,
                    Err(err) if attempts.peek().is_none() => {
                        return Err(err).wrap_err("All address conversions failed")
                    }
                    Err(_) => continue,
                }
            }
            return Err(eyre!("No listen addresses configured"));
        };

        webbrowser::open(url.as_str()).wrap_err("Failed to open browser")?;

        println!("Opened admin-dashboard at {}", url);
        Ok(())
    }
}
