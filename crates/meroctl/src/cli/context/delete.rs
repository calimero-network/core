use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::DeleteContextResponse;
use clap::Parser;
use comfy_table::{Cell, Table};
use eyre::{OptionExt, Result as EyreResult};
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, resolve_alias, RequestType};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Delete a context")]
pub struct DeleteCommand {
    #[clap(name = "CONTEXT", help = "The context to delete")]
    pub context: Alias<ContextId>,
}

impl Report for DeleteContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Context Deletion Status").fg(comfy_table::Color::Blue)
        ]);
        let _ = table.add_row(vec![if self.data.is_deleted {
            Cell::new("✓ Deleted").fg(comfy_table::Color::Green)
        } else {
            Cell::new("✗ Not Deleted").fg(comfy_table::Color::Red)
        }]);
        println!("{table}");
    }
}

impl DeleteCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let connection = environment
            .connection
            .as_ref()
            .ok_or_eyre("No connection configured")?;

    

        let context_id = resolve_alias(&connection.api_url, connection.auth_key.as_ref().unwrap(), self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let mut url = connection.api_url.clone();
        url.set_path(&format!("admin-api/dev/contexts/{}", context_id));

        let response: DeleteContextResponse = do_request(
            &Client::new(),
            url,
            None::<()>,
            connection.auth_key.as_ref(),
            RequestType::Delete,
        )
        .await?;

        environment.output.write(&response);

        Ok(())
    }
}
