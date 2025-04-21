use calimero_server_primitives::admin::ListApplicationsResponse;
use clap::Parser;
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::{PrettyTable, Report};

#[derive(Debug, Parser)]
#[command(about = "List installed applications")]
pub struct ListCommand;

impl Report for ListApplicationsResponse {
    fn report(&self) {
        let mut table = PrettyTable::new(&["ID", "Source", "Size", "Blob ID"]);

        for app in &self.data.apps {
            table.add_row(vec![
                app.id.to_string(),
                app.source.to_string(),
                app.size.to_string(),
                app.blob.to_string(),
            ]);
        }

        table.print();
    }
}

impl ListCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

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
