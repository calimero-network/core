use std::borrow::Cow;

use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::SyncContextResponse;
use clap::Parser;
use eyre::{OptionExt, Result};

use crate::cli::Environment;
use crate::common::resolve_alias;
use crate::output::Report;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Explicitly request a sync")]
pub struct SyncCommand {
    #[clap(long, short, help = "The context to sync", default_value = "default")]
    context: Alias<ContextId>,

    #[clap(long, short, help = "Sync all contexts", conflicts_with = "context")]
    all: bool,
}

impl Report for SyncContextResponse {
    fn report(&self) {
        let mut table = comfy_table::Table::new();
        let _ = table.add_row(["Sync requested"]);
        println!("{table}");
    }
}

impl SyncCommand {
    pub async fn run(self, environment: &Environment) -> Result<()> {
        let connection = environment.connection()?;

        let url = if self.all {
            Cow::from("/admin-api/contexts/sync")
        } else {
            let context_id = resolve_alias(connection, self.context, None)
                .await?
                .value()
                .copied()
                .ok_or_eyre("unable to resolve")?;

            format!("/admin-api/contexts/sync/{context_id}").into()
        };

        let response: SyncContextResponse = connection.post(&url, ()).await?;

        environment.output.write(&response);

        Ok(())
    }
}
