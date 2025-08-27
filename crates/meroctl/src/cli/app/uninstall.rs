use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::UninstallApplicationResponse;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::Result;

use crate::cli::Environment;
use crate::output::Report;

#[derive(Copy, Clone, Parser, Debug)]
#[command(about = "Uninstall an application")]
pub struct UninstallCommand {
    /// Application ID to uninstall
    #[arg(value_name = "APP_ID", help = "application_id of the application")]
    pub app_id: ApplicationId,
}

impl Report for UninstallApplicationResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Application Uninstalled").fg(Color::Green)]);
        let _ = table.add_row(vec![Cell::new(format!(
            "Application ID: {}",
            self.data.application_id
        ))]);
        println!("{table}");
    }
}

impl UninstallCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let response = client.uninstall_application(&self.app_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}
