use calimero_server_primitives::admin::ListApplicationsResponse;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Debug, Parser)]
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
                app.blob.to_string(),
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
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name).await?;

        let response: ListApplicationsResponse = do_request(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/applications")?,
            None::<()>,
            &config.identity,
            RequestType::Get,
        )
        .await?;

        environment.output.write(&response);

        Ok(())
    }
}
