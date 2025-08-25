use calimero_server_primitives::admin::GetContextsResponse;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::Result;

use crate::cli::Environment;
use crate::output::Report;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "List all contexts")]
pub struct ListCommand;

impl Report for GetContextsResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Context ID").fg(Color::Blue),
            Cell::new("Application ID").fg(Color::Blue),
        ]);

        for context in &self.data.contexts {
            let _ = table.add_row(vec![
                context.id.to_string(),
                context.application_id.to_string(),
            ]);
        }
        println!("{table}");
    }
}

impl ListCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let mero_client = environment.mero_client()?;
        let response = mero_client.list_contexts().await?;

        environment.output.write(&response);

        Ok(())
    }
}
