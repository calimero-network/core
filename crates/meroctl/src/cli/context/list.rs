use calimero_server_primitives::admin::GetContextsResponse;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::{OptionExt, Result as EyreResult};
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, RequestType};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "List all contexts")]
pub struct ListCommand;

impl Report for GetContextsResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Contexts").fg(Color::Blue),
            Cell::new("ID").fg(Color::Blue),
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
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let connection = environment
            .connection
            .as_ref()
            .ok_or_eyre("No connection configured")?;

        let mut url = connection.api_url.clone();
        url.set_path("admin-api/dev/contexts");

        let response: GetContextsResponse = do_request(
            &Client::new(),
            url,
            None::<()>,
            connection.auth_key.as_ref(),
            RequestType::Get,
        )
        .await?;

        environment.output.write(&response);

        Ok(())
    }
}
