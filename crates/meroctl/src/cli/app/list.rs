use calimero_server_primitives::admin::ListApplicationsResponse;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::Result;

use crate::cli::Environment;
use crate::output::Report;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "List installed applications")]
pub struct ListCommand;

impl Report for ListApplicationsResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Application ID").fg(Color::Blue),
            Cell::new("Source").fg(Color::Blue),
            Cell::new("Size").fg(Color::Blue),
            Cell::new("Blob ID").fg(Color::Blue),
        ]);

        for app in &self.data.apps {
            let _ = table.add_row(vec![
                app.id.to_string(),
                app.source.to_string(),
                format!("{} bytes", app.size),
                app.blob.bytecode.to_string(),
            ]);
        }
        println!("{table}");

        for app in &self.data.apps {
            if !app.metadata.is_empty() {
                let mut meta_table = Table::new();
                let _ = meta_table.set_header(vec![
                    Cell::new(format!("Metadata for {}", app.id)).fg(Color::Green)
                ]);

                for data in &app.metadata {
                    let _ = meta_table.add_row(vec![format!("{:?}", data)]);
                }
                println!("{meta_table}");
            }
        }
    }
}

impl ListCommand {
    pub async fn run(self, environment: &Environment) -> Result<()> {
        let connection = environment.connection()?;

        let response: ListApplicationsResponse = connection.get("admin-api/applications").await?;

        environment.output.write(&response);

        Ok(())
    }
}
