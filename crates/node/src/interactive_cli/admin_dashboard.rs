// crates/interactive_cli/src/admin_dashboard.rs
use clap::Args;
use eyre::Result;
use webbrowser;

#[derive(Debug, Args)]
pub struct AdminDashboardCommand {
    #[clap(required = true)]
    port: u16,
}

impl AdminDashboardCommand {
    pub fn run(&self) -> Result<()> {
        let url = format!("http://localhost:{}/admin-dashboard", self.port);
        webbrowser::open(&url)
            .map_err(|e| eyre::eyre!("Failed to open browser: {}", e))?;
        println!("Opened admin dashboard at {}", url);
        Ok(())
    }
}