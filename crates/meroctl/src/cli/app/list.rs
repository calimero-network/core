use calimero_server_primitives::admin::ListApplicationsResponse;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::{eyre, Result as EyreResult};
use reqwest::Client;

use crate::cli::{ConnectionInfo, Environment};
use crate::common::{do_request, multiaddr_to_url, RequestType};
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
        let (url, keypair) = match &environment.connection {
            Some(ConnectionInfo::Local { config, multiaddr }) => (
                multiaddr_to_url(multiaddr, "admin-api/dev/applications")?,
                Some(&config.identity),
            ),
            Some(ConnectionInfo::Remote { api }) => {
                let mut url = api.clone();
                url.set_path("admin-api/dev/applications");
                (url, None)
            }
            None => return Err(eyre!("No connection configured")),
        };

        let response: ListApplicationsResponse =
            do_request(&Client::new(), url, None::<()>, keypair, RequestType::Get).await?;

        environment.output.write(&response);

        Ok(())
    }
}
