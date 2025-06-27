use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::DeleteContextResponse;
use clap::Parser;
use comfy_table::{Cell, Table};
use eyre::{OptionExt, Result as EyreResult};

use crate::cli::Environment;
use crate::common::resolve_alias;
use crate::output::Report;

#[derive(Copy, Clone, Debug, Parser)]
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
        let connection = environment.connection()?;

        let context_id = resolve_alias(connection, self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let response: DeleteContextResponse = connection
            .delete(&format!("admin-api/contexts/{}", context_id))
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
